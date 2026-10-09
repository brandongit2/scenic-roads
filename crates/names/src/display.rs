//! The display rule (docs/plan.md §7) and the translation lines it reads.
//!
//! A name shows as a **main** label and an optional smaller **sub** line. A thing's English, in
//! order:
//! 1. its own, from a source about that thing (OSM's `name:en`, its romanised name, its kana
//!    reading by rule, its Wikidata label, its Wikipedia title, a register's English): the name as
//!    main, that English as sub. It belongs to that thing alone, and wins even when wrong;
//! 2. its name's translation: the line for its name, kind and language, the languages tried in
//!    order: those OSM gives the name, then those spoken where it is ([`crate::spoken`]);
//! 3. else none: the name alone (and it goes on the to-do list).
//!
//! Sub is dropped when it is empty, already one of main's parts ("Alba / Scotland" and "Scotland"),
//! or the same name but for accents, case, punctuation or spacing (Montréal, Montreal).
//!
//! The lines are read from a translations folder (the server's local copy of `translations/` on
//! the NAS): every `*.jsonl` under it but `todo/`, lines `{"n", "kind", "langs", "main", "sub",
//! "via"}`. Where lines share a name, kind and language, the later file (by path) wins, and within
//! a file the later line. Lines not done (`via` "todo" or "skipped") are left out, and so are the
//! area tables' lines (no kind or languages), counted ([`Summary::old`]).
//!
//! [`Names::refresh`] picks up dropped, replaced and removed files, reading a file only once its
//! size and modification time have held for ten seconds, so a file still being copied is never
//! read. Lookups never touch the disk: refresh a clone (cheap, the tables are shared) and swap it
//! in, so requests never wait on the NAS.

use crate::spoken::{lookup_order, Lang, Spoken};
use crate::table::{self, Table};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufReader, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// How long a file's size and modification time must hold before it is read.
pub const STABLE: Duration = Duration::from_secs(10);

/// Files read at once (the NAS serves a few streams faster than one).
const READERS: usize = 4;

/// Hashed into every language's version: bump it when the display rule or the reading of the
/// files changes, so ETags made under the old rule stop matching. (3: lines by name, kind and
/// language.)
const RULES: &[u8] = b"names display rule 3";

/// What a name names, as translations are filed: the same words can be a hamlet that keeps its
/// name and a mill to translate ("Moulin").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Road,
    /// A city, town, village, hamlet, suburb, quarter, neighbourhood or isolated dwelling.
    Settlement,
    /// Everything else named on the map: water, parks, peaks, stations, lines, sights, regions.
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Road => "road",
            Kind::Settlement => "settlement",
            Kind::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "road" => Some(Kind::Road),
            "settlement" => Some(Kind::Settlement),
            "other" => Some(Kind::Other),
            _ => None,
        }
    }

    /// The kind of a place by its OSM `place` value (OpenMapTiles' `class` in its `place` layer):
    /// settlements city to hamlet and their parts; anything else (a country, a state, an island, a
    /// locality) is other.
    pub fn of_place(class: &str) -> Kind {
        match class {
            "city" | "town" | "village" | "hamlet" | "suburb" | "quarter" | "neighbourhood" | "isolated_dwelling" | "borough" | "farm" => Kind::Settlement,
            _ => Kind::Other,
        }
    }
}

/// Whether two names are the same but for accents, case, punctuation or spacing ("Montréal" and
/// "Montreal", "Mont-Blanc" and "Mont Blanc"): `web/src/english.ts`'s `sameName`, ported exactly.
///
/// Both sides are decomposed (NFKD), stripped of combining marks (`\p{M}`), lower-cased (the full
/// Unicode mapping, final sigma included, as JavaScript's `toLowerCase`), and cut down to letters
/// and numbers (`\p{L}` and `\p{N}`).
pub fn same_name(a: &str, b: &str) -> bool {
    a == b || norm(a) == norm(b)
}

fn norm(s: &str) -> String {
    let unmarked: String = s.nfkd().filter(|c| !is_combining_mark(*c)).collect();
    unmarked.to_lowercase().chars().filter(|c| letter_or_number(*c)).collect()
}

/// `\p{L}` or `\p{N}`. Rust's `is_alphanumeric` is the `Alphabetic` property or `N`, and
/// `Alphabetic` also holds combining marks (vowel signs and the like, Other_Alphabetic) and symbols:
/// the circled and squared Latin letters. Marks are gone by now (and dropped again here, should
/// lower-casing bring one back); NFKD turns most of the symbols into plain letters, which leaves
/// the 52 negative circled and squared ones (🅐, 🅰), which `\p{L}` doesn't hold. Checked against
/// Node for every code point (`norm_matches_node`).
fn letter_or_number(c: char) -> bool {
    c.is_alphanumeric() && !is_combining_mark(c) && !matches!(c as u32, 0x1F150..=0x1F169 | 0x1F170..=0x1F189)
}

/// A name as shown: the label, and the smaller line under it (in text: "main (sub)").
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Display {
    pub main: String,
    pub sub: Option<String>,
}

/// [`Display`] borrowed from the name, the thing's own English and the tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DisplayRef<'a> {
    pub main: &'a str,
    pub sub: Option<&'a str>,
}

impl<'a> DisplayRef<'a> {
    /// Applies the sub rule: no sub when it is empty, already a part of main ([`in_parts`]), or the
    /// same name as main ([`same_name`]).
    pub fn new(main: &'a str, sub: Option<&'a str>) -> Self {
        let sub = sub.filter(|s| !s.trim().is_empty() && !in_parts(main, s) && !same_name(s, main));
        DisplayRef { main, sub }
    }

    pub fn to_display(self) -> Display {
        Display { main: self.main.to_owned(), sub: self.sub.map(str::to_owned) }
    }
}

/// The separators between the parts of a name that holds several ("Alba / Scotland",
/// "Bolzano - Bozen", "Mont Blanc (Monte Bianco)").
const PARTS: [&str; 5] = [" / ", ";", " - ", "(", ")"];

/// Whether `sub` is one of `main`'s parts: `main` split on `" / "`, `";"`, `" - "`, `"("` and `")"`,
/// each part trimmed and compared ignoring case. Whole parts only, so "Ma" isn't in "Mapo".
pub fn in_parts(main: &str, sub: &str) -> bool {
    let sub = sub.trim().to_lowercase();
    let mut parts = vec![main];
    for sep in PARTS {
        parts = parts.into_iter().flat_map(|p| p.split(sep)).collect();
    }
    parts.iter().any(|p| p.trim().to_lowercase() == sub)
}

/// A name's translation line, as written (before the sub rule), and the language it was found in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation<'a> {
    pub main: &'a str,
    pub sub: Option<&'a str>,
    pub lang: Lang,
}

/// What the folder holds, for the status and the logs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Files with lines read (not those of the area tables' lines alone).
    pub files: usize,
    /// Distinct names per file, summed.
    pub names: usize,
    /// Lines kept.
    pub lines: usize,
    /// Lines left out as not done (`via` "todo" or "skipped").
    pub ignored: usize,
    /// The area tables' lines (no kind or languages), left out.
    pub old: usize,
    /// The languages lines hold for, with each one's version.
    pub langs: BTreeMap<String, u64>,
}

impl Summary {
    /// The folder has the area tables' lines and none by language: a server with this loader on
    /// the old files only would show no translation at all.
    pub fn only_old(&self) -> bool {
        self.lines == 0 && self.old > 0
    }
}

/// A `.jsonl` file under the folder, as last seen.
#[derive(Clone, Debug)]
struct Tracked {
    /// Size and modification time when last seen, and since when (by the monotonic clock).
    seen: Stat,
    since: Instant,
    /// Seen at the first scan already ten seconds old by its own modification time.
    aged: bool,
    /// Reading it (as `seen`) failed: tried again at each scan, warned about once.
    failed: bool,
    /// The table in use, the stat it was read at, and its report's counts (not done, old).
    loaded: Option<(Stat, Arc<Table>, u64, u64)>,
}

/// The translation lines of a translations folder.
#[derive(Clone)]
pub struct Names {
    dir: PathBuf,
    stable: Duration,
    scanned: bool,
    files: BTreeMap<String, Tracked>,
    /// The tables in path order (later wins).
    tables: Vec<Arc<Table>>,
    versions: BTreeMap<Lang, u64>,
    warnings: Vec<String>,
}

impl fmt::Debug for Names {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Names").field("dir", &self.dir).field("summary", &self.summary()).field("pending", &self.pending()).finish()
    }
}

/// What reading one file came to.
enum Outcome {
    Read(Table, table::Report),
    /// It changed while being read: wait for it to hold again.
    Moved(Stat),
    Gone,
    Failed(anyhow::Error),
}

impl Names {
    /// Lines for `dir` with nothing read yet: the first [`refresh`](Self::refresh) reads what is
    /// there as [`load`](Self::load) would. For starting while the NAS is away.
    pub fn new(dir: &Path) -> Names {
        Names { dir: dir.to_owned(), stable: STABLE, scanned: false, files: BTreeMap::new(), tables: Vec::new(), versions: BTreeMap::new(), warnings: Vec::new() }
    }

    /// Reads every `*.jsonl` file under `dir`, skipping hidden files and folders and `todo/`.
    ///
    /// With no history yet, a file counts as settled when its modification time is at least ten
    /// seconds old; one modified more recently waits for [`refresh`](Self::refresh) to see it hold.
    /// Fails only when the folder can't be listed.
    pub fn load(dir: &Path) -> Result<Names> {
        let mut names = Names::new(dir);
        names.refresh()?;
        Ok(names)
    }

    /// Looks at the folder again: reads new and changed files once their size and modification
    /// time have held for ten seconds, and drops the tables of removed files. Unchanged files are
    /// not read again; a file that fails to read keeps its previous table (with a warning). Returns
    /// whether any table changed (and with it some language's [`version`](Self::version)). Fails,
    /// with nothing changed, when the folder can't be listed.
    pub fn refresh(&mut self) -> Result<bool> {
        self.scan(Instant::now(), SystemTime::now())
    }

    fn scan(&mut self, now: Instant, wall: SystemTime) -> Result<bool> {
        let found = list(&self.dir, &mut self.warnings)?;
        let first = !self.scanned;
        self.scanned = true;
        let mut changed = false;
        self.files.retain(|rel, t| {
            let keep = found.contains_key(rel);
            changed |= !keep && t.loaded.is_some();
            keep
        });

        let mut jobs = Vec::new();
        for (rel, (path, stat)) in found {
            match self.files.get_mut(&rel) {
                Some(t) if t.seen == stat => {}
                Some(t) => (t.seen, t.since, t.aged, t.failed) = (stat, now, false, false),
                None => {
                    let aged = first && stat.age(wall) >= self.stable;
                    self.files.insert(rel.clone(), Tracked { seen: stat, since: now, aged, failed: false, loaded: None });
                }
            }
            let Some(t) = self.files.get(&rel) else { continue };
            let current = t.loaded.as_ref().is_some_and(|l| l.0 == stat);
            let settled = t.aged || now.saturating_duration_since(t.since) >= self.stable;
            if !current && settled {
                jobs.push((rel, path, stat));
            }
        }

        let outcomes = read_all(&jobs);
        // The area tables' files read now: told once, together.
        let mut old_files: Vec<String> = Vec::new();
        let mut old_lines = 0u64;
        for ((rel, _, stat), outcome) in jobs.into_iter().zip(outcomes) {
            match outcome {
                Outcome::Read(table, report) => {
                    if report.malformed > 0 {
                        let first = report.first_malformed.unwrap_or(0);
                        self.warnings.push(format!("{rel}: {} malformed lines skipped (first: line {first})", report.malformed));
                    }
                    if report.unfinished {
                        self.warnings.push(format!("{rel}: unfinished last line ignored"));
                    }
                    if report.old > 0 {
                        old_files.push(rel.clone());
                        old_lines += report.old;
                    }
                    if let Some(t) = self.files.get_mut(&rel) {
                        t.loaded = Some((stat, Arc::new(table), report.ignored, report.old));
                        t.failed = false;
                        changed = true;
                    }
                }
                Outcome::Moved(now_stat) => {
                    if let Some(t) = self.files.get_mut(&rel) {
                        (t.seen, t.since, t.aged, t.failed) = (now_stat, now, false, false);
                    }
                }
                Outcome::Gone => {
                    if let Some(t) = self.files.remove(&rel) {
                        changed |= t.loaded.is_some();
                    }
                }
                Outcome::Failed(e) => {
                    let Some(t) = self.files.get_mut(&rel) else { continue };
                    if !t.failed {
                        let kept = if t.loaded.is_some() { "kept its previous table" } else { "not read" };
                        self.warnings.push(format!("{rel}: {e:#}; {kept}, tried again at each refresh"));
                    }
                    t.failed = true;
                }
            }
        }
        if !old_files.is_empty() {
            let shown: Vec<&str> = old_files.iter().take(3).map(String::as_str).collect();
            let more = if old_files.len() > 3 { format!(" and {} more", old_files.len() - 3) } else { String::new() };
            self.warnings.push(format!("{old_lines} lines of the area tables' format (no kind or languages) left out, in {} files ({}{more})", old_files.len(), shown.join(", ")));
        }
        if changed {
            self.rebuild();
        }
        Ok(changed)
    }

    fn rebuild(&mut self) {
        let mut keys: BTreeMap<Lang, Vec<u8>> = BTreeMap::new();
        self.tables.clear();
        for (rel, t) in &self.files {
            let Some((stat, table, _, _)) = &t.loaded else { continue };
            // (A file of the area tables' lines alone is passed over: no empty table kept.)
            if table.lines() == 0 {
                continue;
            }
            self.tables.push(table.clone());
            for l in table.langs() {
                let key = keys.entry(*l).or_insert_with(|| RULES.to_vec());
                key.extend_from_slice(rel.as_bytes());
                key.push(0);
                key.extend_from_slice(&stat.size.to_le_bytes());
                key.extend_from_slice(&stat.mtime.to_le_bytes());
            }
        }
        self.versions = keys.into_iter().map(|(l, k)| (l, table::hash(&k))).collect();
    }

    /// The folder read.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A hash of the files holding lines in `lang`: of their relative paths, sizes and
    /// modification times (and the display rule). It changes whenever any of them does and holds
    /// otherwise, across runs too: for HTTP ETags of anything whose names may be read in `lang`.
    pub fn version(&self, lang: Lang) -> u64 {
        self.versions.get(&lang).copied().unwrap_or_else(|| table::hash(RULES))
    }

    /// The versions of `langs` together.
    pub fn version_of(&self, langs: &[Lang]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ table::hash(RULES);
        for l in langs {
            h ^= self.version(*l);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h
    }

    /// The versions of every language with lines, together: for what spans the world.
    pub fn version_all(&self) -> u64 {
        self.version_of(&self.versions.keys().copied().collect::<Vec<_>>())
    }

    /// What the folder holds.
    pub fn summary(&self) -> Summary {
        let mut s = Summary::default();
        for t in self.files.values() {
            let Some((_, table, ignored, old)) = &t.loaded else { continue };
            s.files += usize::from(table.lines() > 0);
            s.names += table.len();
            s.lines += table.lines();
            s.ignored += *ignored as usize;
            s.old += *old as usize;
        }
        s.langs = self.versions.iter().map(|(l, v)| (l.as_str().to_owned(), *v)).collect();
        s
    }

    /// Distinct names per file, summed.
    pub fn entries(&self) -> usize {
        self.tables.iter().map(|t| t.len()).sum()
    }

    /// Heap bytes held by the tables.
    pub fn heap_bytes(&self) -> usize {
        self.tables.iter().map(|t| t.heap_bytes()).sum()
    }

    /// When new or changed files are waiting to settle: how long until the first may be read (zero:
    /// [`refresh`](Self::refresh) now), so a caller polling once a minute can look again sooner.
    /// Files that failed to read aren't waiting: each refresh tries them again.
    pub fn pending(&self) -> Option<Duration> {
        let now = Instant::now();
        self.files
            .values()
            .filter(|t| !t.failed && !t.loaded.as_ref().is_some_and(|l| l.0 == t.seen))
            .map(|t| if t.aged { Duration::ZERO } else { self.stable.saturating_sub(now.saturating_duration_since(t.since)) })
            .min()
    }

    /// The warnings since the last call: malformed lines, unfinished last lines, the area tables'
    /// lines, files that failed to read.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// The line for a `kind` of name in the first of `langs` that has one, as written; the latest
    /// file's, the latest line's within it.
    pub fn translation<'a>(&'a self, kind: Kind, name: &'a str, langs: &[Lang]) -> Option<Translation<'a>> {
        if self.tables.is_empty() || langs.is_empty() {
            return None;
        }
        let h = table::hash(name.as_bytes());
        for &lang in langs {
            for t in self.tables.iter().rev() {
                if let Some(r) = t.get(h, name.as_bytes(), kind, lang) {
                    return Some(Translation { main: r.main.unwrap_or(name), sub: r.sub, lang });
                }
            }
        }
        None
    }

    /// Whether a `kind` of name has a line in any of `langs`.
    pub fn has_line(&self, kind: Kind, name: &str, langs: &[Lang]) -> bool {
        self.translation(kind, name, langs).is_some()
    }

    /// How a `kind` of name shows: `own_en` is the thing's own English (step 1), `osm` the
    /// languages OSM gives its name, `here` the languages spoken where it is. An own English that
    /// is the name itself ([`same_name`]: OpenMapTiles' `name_en` falls back to the name) is none.
    pub fn display<'a>(&'a self, kind: Kind, name: &'a str, own_en: Option<&'a str>, osm: &[Lang], here: &[Lang]) -> DisplayRef<'a> {
        if let Some(en) = own_en.filter(|e| !e.trim().is_empty() && !same_name(e, name)) {
            return DisplayRef::new(name, Some(en));
        }
        match self.translation(kind, name, &lookup_order(osm, here)) {
            Some(t) => DisplayRef::new(t.main, t.sub),
            None => DisplayRef::new(name, None),
        }
    }
}

/// The lines and the languages spoken where: what naming a thing somewhere needs.
#[derive(Clone, Debug)]
pub struct Namer {
    pub names: Names,
    /// None until the pass's outlines have been read: then only OSM's own language tags lead to a
    /// line.
    pub spoken: Option<Arc<Spoken>>,
}

impl Namer {
    /// The languages spoken at a point (none known: none).
    pub fn here(&self, lon: f64, lat: f64) -> &[Lang] {
        self.spoken.as_deref().map_or(&[][..], |s| s.langs_at(lon, lat))
    }

    /// How a `kind` of name shows at `lon`, `lat` ([`Names::display`]).
    pub fn display_at<'a>(&'a self, kind: Kind, name: &'a str, own_en: Option<&'a str>, osm: &[Lang], lon: f64, lat: f64) -> DisplayRef<'a> {
        self.names.display(kind, name, own_en, osm, self.here(lon, lat))
    }

    fn spoken_version(&self) -> u64 {
        self.spoken.as_ref().map_or(0, |s| s.version())
    }

    /// A version for what shows names within a box (degrees): the versions of the languages spoken
    /// there, and the raster's. (A line in another language, which only OSM's tag on a thing there
    /// would lead to, shows once the tile is asked for again for another reason.) Before the raster
    /// is known: every language's.
    pub fn version_in(&self, west: f64, south: f64, east: f64, north: f64) -> u64 {
        let v = match &self.spoken {
            Some(s) => self.names.version_of(&s.langs_in(west, south, east, north)),
            None => self.names.version_all(),
        };
        v ^ self.spoken_version().rotate_left(1)
    }

    /// A version over every language (for what spans the world).
    pub fn version_all(&self) -> u64 {
        self.names.version_all() ^ self.spoken_version().rotate_left(1)
    }
}

/// A file's size and modification time (nanoseconds from 1970, negative before).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stat {
    size: u64,
    mtime: i128,
}

impl Stat {
    fn of(md: &fs::Metadata) -> Stat {
        let mtime = md.modified().map_or(0, |t| match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i128,
            Err(e) => -(e.duration().as_nanos() as i128),
        });
        Stat { size: md.len(), mtime }
    }

    /// How long ago, by `now`, the file was last modified (zero if in the future).
    fn age(&self, now: SystemTime) -> Duration {
        let now = match now.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i128,
            Err(e) => -(e.duration().as_nanos() as i128),
        };
        let ns = (now - self.mtime).clamp(0, u64::MAX as i128) as u64;
        Duration::from_nanos(ns)
    }
}

/// The `.jsonl` files under `dir` by relative path ('/'-separated), with where they are and their
/// stat. Hidden entries, `todo` folders and symbolic links to folders are passed over.
fn list(dir: &Path, warnings: &mut Vec<String>) -> Result<BTreeMap<String, (PathBuf, Stat)>> {
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, (PathBuf, Stat)>, warnings: &mut Vec<String>) -> Result<()> {
        let entries = fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("listing {}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                warnings.push(format!("{}: skipped, its name isn't UTF-8", path.display()));
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let kind = entry.file_type().with_context(|| format!("reading {}", path.display()))?;
            if kind.is_dir() {
                if name != "todo" {
                    walk(&path, &format!("{prefix}{name}/"), out, warnings)?;
                }
            } else if name.ends_with(".jsonl") {
                let md = match fs::metadata(&path) {
                    Ok(md) => md,
                    Err(e) if e.kind() == ErrorKind::NotFound => continue,
                    Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
                };
                if md.is_file() {
                    out.insert(format!("{prefix}{name}"), (path, Stat::of(&md)));
                }
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, "", &mut out, warnings)?;
    Ok(out)
}

/// Reads the files, a few at a time.
fn read_all(jobs: &[(String, PathBuf, Stat)]) -> Vec<Outcome> {
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, Outcome)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..READERS.min(jobs.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((_, path, stat)) = jobs.get(i) else { break };
                        mine.push((i, read_one(path, *stat)));
                    }
                    mine
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
    });
    done.sort_by_key(|(i, _)| *i);
    let mut done = done.into_iter().peekable();
    (0..jobs.len())
        .map(|i| match done.next_if(|(j, _)| *j == i) {
            Some((_, o)) => o,
            None => Outcome::Failed(anyhow::anyhow!("its reader stopped")),
        })
        .collect()
}

/// Reads one file, checking afterwards that it is still the file that was seen settled.
fn read_one(path: &Path, stat: Stat) -> Outcome {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Outcome::Gone,
        Err(e) => return Outcome::Failed(anyhow::Error::new(e).context("opening")),
    };
    let (table, report) = match Table::read(&mut BufReader::with_capacity(1 << 20, file)) {
        Ok(read) => read,
        Err(e) => return Outcome::Failed(e.context("reading")),
    };
    match fs::metadata(path) {
        Ok(md) if Stat::of(&md) != stat || report.bytes != stat.size => Outcome::Moved(Stat::of(&md)),
        Ok(_) => Outcome::Read(table, report),
        Err(e) if e.kind() == ErrorKind::NotFound => Outcome::Gone,
        Err(e) => Outcome::Failed(anyhow::Error::new(e).context("checking")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::Dir;

    #[test]
    fn same_names() {
        assert!(same_name("Montréal", "Montreal"));
        assert!(same_name("Mont-Blanc", "mont blanc"));
        assert!(same_name("Saint-Jean-sur-Richelieu", "Saint Jean sur Richelieu"));
        assert!(same_name("ﬁord", "FIORD")); // NFKD unties the ligature
        assert!(same_name("Ａｂｃ１", "abc1")); // full-width forms
        assert!(same_name("Ⅻ", "xii")); // a letter-like numeral decomposes to letters
        assert!(!same_name("Straße", "STRASSE")); // lower-casing, not case folding
        assert!(same_name("ΟΔΟΣ", "οδο\u{3c2}")); // a final capital sigma lower-cases to ς
        assert!(!same_name("ΟΔΟΣ", "οδο\u{3c3}"));
        assert!(same_name("İstanbul", "istanbul")); // the dot İ leaves behind is a mark
        assert!(same_name("がっこう", "かっこう")); // dakuten are marks
        assert!(same_name("🅰🅱", "")); // negative squared letters are symbols, not letters
        assert!(!same_name("中山", "Zhongshan"));
        assert!(!same_name("Saint-Jean", "Saint-Jeanne"));
        assert!(same_name("", "-"));
    }

    /// `norm` against english.ts's own expression in Node, for every code point and some strings
    /// whose context matters (final sigma). Opt-in (`cargo test -p names -- --ignored`): it needs
    /// Node, and a Node on a newer Unicode than Rust's differs on newly assigned characters.
    #[test]
    #[ignore]
    fn norm_matches_node() {
        const TRICKY: &[&str] = &["ΟΔΟΣ", "ΣΑΣ", "Σ", "ΑΣ.", "ΑΣ Α", "Α.Σ", "ΑΣ\u{301}", "İSTANBUL", "Ǆemal", "ǅ", "Ⅻ ⅻ", "ﬀ ﬃ ẞ", "Ｔｏｋｙｏ", "①②", "がっこう", "Ꭰꭰ", "𝔄𝔟𝔠", "🅰🅱🄰"];
        let script = r#"
            const norm = (s) => s.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, '');
            const hex = (s) => [...s].map((c) => c.codePointAt(0).toString(16)).join(' ');
            const out = [];
            for (let cp = 0; cp <= 0x10ffff; cp++) {
              if (cp >= 0xd800 && cp <= 0xdfff) continue;
              const n = norm(String.fromCodePoint(cp));
              if (n) out.push(cp.toString(16) + '\t' + hex(n));
            }
            for (const s of JSON.parse(process.argv[1])) out.push('s' + hex(s) + '\t' + hex(norm(s)));
            out.push('unicode ' + process.versions.unicode);
            process.stdout.write(out.join('\n'));
        "#;
        let tricky = serde_json::to_string(TRICKY).expect("json");
        let Ok(out) = std::process::Command::new("node").arg("-e").arg(script).arg(&tricky).output() else {
            eprintln!("no node: skipped");
            return;
        };
        assert!(out.status.success(), "node: {}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8(out.stdout).expect("utf-8");
        let hex = |s: &str| s.chars().map(|c| format!("{:x}", c as u32)).collect::<Vec<_>>().join(" ");
        let unhex = |h: &str| h.split(' ').filter(|x| !x.is_empty()).map(|x| char::from_u32(u32::from_str_radix(x, 16).expect("hex")).expect("char")).collect::<String>();
        let mut nonempty = std::collections::HashMap::new();
        let mut mismatches = Vec::new();
        let mut node_unicode = String::new();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("unicode ") {
                node_unicode = v.to_owned();
            } else if let Some(rest) = line.strip_prefix('s') {
                let (s, n) = rest.split_once('\t').expect("tab");
                let s = unhex(s);
                if hex(&norm(&s)) != n {
                    mismatches.push(format!("{s:?}: node {n}, rust {}", hex(&norm(&s))));
                }
            } else {
                let (cp, n) = line.split_once('\t').expect("tab");
                nonempty.insert(u32::from_str_radix(cp, 16).expect("hex"), n.to_owned());
            }
        }
        assert!(nonempty.len() > 100_000, "node gave {} code points", nonempty.len());
        for cp in (0..=0x10ffffu32).filter(|cp| !(0xd800..=0xdfff).contains(cp)) {
            let c = char::from_u32(cp).expect("char");
            let rust = hex(&norm(&c.to_string()));
            let node = nonempty.get(&cp).map_or("", String::as_str);
            if rust != node {
                mismatches.push(format!("U+{cp:04X}: node {node:?}, rust {rust:?}"));
            }
        }
        // A Node on a newer Unicode keeps letters Rust doesn't know yet: only those may differ.
        let (ma, mi, _) = char::UNICODE_VERSION;
        if node_unicode != format!("{ma}.{mi}") {
            let before = mismatches.len();
            mismatches.retain(|m| !m.ends_with(", rust \"\""));
            eprintln!("Unicode {node_unicode} in Node, {ma}.{mi} in Rust: {} code points new to Node", before - mismatches.len());
        }
        assert!(mismatches.is_empty(), "{} mismatches: {:?}", mismatches.len(), &mismatches[..mismatches.len().min(40)]);
    }

    #[test]
    fn sub_rule() {
        let d = |main, sub| DisplayRef::new(main, sub).sub;
        assert_eq!(d("Montréal", Some("Montreal")), None);
        // Already a part of main: whole parts, any case.
        assert_eq!(d("Alba / Scotland", Some("Scotland")), None);
        assert_eq!(d("Alba / Scotland", Some(" scotland ")), None);
        assert_eq!(d("ALBA / SCOTLAND", Some("Alba")), None);
        assert_eq!(d("Bolzano - Bozen", Some("Bozen")), None);
        assert_eq!(d("Seoul;Séoul", Some("SÉOUL")), None);
        assert_eq!(d("Mont Blanc (Monte Bianco)", Some("Monte Bianco")), None);
        assert_eq!(d("Mapo", Some("Ma")), Some("Ma"));
        assert_eq!(d("Saint-Jean", Some("Jean")), Some("Jean"));
        assert_eq!(d("Alba/Scotland", Some("Scotland")), Some("Scotland")); // only " / " splits
        assert_eq!(d("Rua do Porto", Some("Port")), Some("Port"));
        assert_eq!(d("Lac Saint-Jean", Some("Lake Saint-Jean")), Some("Lake Saint-Jean"));
        assert_eq!(d("Lac", Some("")), None);
        assert_eq!(d("Lac", Some("  ")), None);
        assert_eq!(d("Lac", None), None);
    }

    fn l(s: &str) -> Lang {
        Lang::parse(s).unwrap()
    }

    fn ls(v: &[&str]) -> Vec<Lang> {
        v.iter().map(|s| l(s)).collect()
    }

    fn show(names: &Names, kind: Kind, name: &str, own: Option<&str>, osm: &[&str], here: &[&str]) -> (String, Option<String>) {
        let d = names.display(kind, name, own, &ls(osm), &ls(here));
        (d.main.to_owned(), d.sub.map(str::to_owned))
    }

    fn ds(main: &str, sub: Option<&str>) -> (String, Option<String>) {
        (main.to_owned(), sub.map(str::to_owned))
    }

    const QC: &[&str] = &["fr", "en"];
    const FR: &[&str] = &["fr"];
    const JP: &[&str] = &["ja"];
    const TW: &[&str] = &["zh", "nan", "hak"];
    const HK: &[&str] = &["yue", "en"];

    fn line(n: &str, kind: &str, langs: &str, main: Option<&str>, sub: Option<&str>) -> String {
        serde_json::json!({"n": n, "kind": serde_json::from_str::<serde_json::Value>(kind).unwrap(), "langs": serde_json::from_str::<serde_json::Value>(langs).unwrap(), "main": main, "sub": sub, "via": "test"}).to_string() + "\n"
    }

    #[test]
    fn the_rule() {
        let d = Dir::new();
        let mut text = String::new();
        text += &line("中山", "\"other\"", "[\"zh\"]", None, Some("Zhongshan"));
        text += &line("中山", "\"other\"", "[\"ja\"]", None, Some("Nakayama"));
        text += &line("中山", "\"road\"", "[\"ja\"]", None, Some("Naka-yama"));
        text += &line("中山", "\"other\"", "[\"yue\"]", None, Some("Chung Shan"));
        text += &line("Lac Bleu", "\"other\"", "[\"fr\"]", None, Some("Blue Lake"));
        text += &line("Moulin", "\"settlement\"", "[\"fr\"]", None, None);
        text += &line("Moulin", "\"other\"", "[\"fr\"]", Some("Mill"), None);
        text += &line("Kêr Vraz", "[\"settlement\", \"other\"]", "[\"br\"]", None, Some("Big Village"));
        text += &line("Église", "[\"settlement\", \"other\"]", "[\"fr\"]", Some("Church"), None);
        d.write("0-converted/all.jsonl", &text);
        let names = Names::load(&d.0).expect("load");

        // Each language its own line: Taiwan, Japan, Hong Kong.
        assert_eq!(show(&names, Kind::Other, "中山", None, &[], TW), ds("中山", Some("Zhongshan")));
        assert_eq!(show(&names, Kind::Other, "中山", None, &[], JP), ds("中山", Some("Nakayama")));
        assert_eq!(show(&names, Kind::Road, "中山", None, &[], JP), ds("中山", Some("Naka-yama")));
        assert_eq!(show(&names, Kind::Other, "中山", None, &[], HK), ds("中山", Some("Chung Shan")));
        // OSM's language first: a name tagged Japanese in Taiwan reads as Japanese; one tagged
        // Chinese in Hong Kong reads as Cantonese there.
        assert_eq!(show(&names, Kind::Other, "中山", None, &["ja"], TW), ds("中山", Some("Nakayama")));
        assert_eq!(show(&names, Kind::Other, "中山", None, &["zh"], HK), ds("中山", Some("Chung Shan")));
        // No line in a kind: nothing from another kind's.
        assert_eq!(show(&names, Kind::Road, "中山", None, &[], TW), ds("中山", None));
        assert_eq!(show(&names, Kind::Settlement, "中山", None, &[], JP), ds("中山", None));
        // One French line, used in France and Quebec alike.
        assert_eq!(show(&names, Kind::Other, "Lac Bleu", None, &[], FR), ds("Lac Bleu", Some("Blue Lake")));
        assert_eq!(show(&names, Kind::Other, "Lac Bleu", None, &[], QC), ds("Lac Bleu", Some("Blue Lake")));
        assert_eq!(show(&names, Kind::Other, "Lac Bleu", None, &[], &["en"]), ds("Lac Bleu", None));
        // A hamlet keeps its name; a mill is translated.
        assert_eq!(show(&names, Kind::Settlement, "Moulin", None, &[], FR), ds("Moulin", None));
        assert_eq!(show(&names, Kind::Other, "Moulin", None, &[], FR), ds("Mill", None));
        // Brittany: Breton after French.
        assert_eq!(show(&names, Kind::Settlement, "Kêr Vraz", None, &[], &["fr", "br"]), ds("Kêr Vraz", Some("Big Village")));
        // The thing's own English wins, even over a line and even when wrong; the sub rule holds.
        assert_eq!(show(&names, Kind::Other, "Église", Some("Leclerc tank"), &[], FR), ds("Église", Some("Leclerc tank")));
        assert_eq!(show(&names, Kind::Other, "Église", Some(" "), &[], FR), ds("Church", None));
        assert_eq!(show(&names, Kind::Settlement, "Montréal", Some("Montreal"), &[], QC), ds("Montréal", None));
        // An "own English" that is the name itself (OpenMapTiles' name_en without name:en) is none:
        // the name's line shows.
        assert_eq!(show(&names, Kind::Other, "Lac Bleu", Some("Lac Bleu"), &[], FR), ds("Lac Bleu", Some("Blue Lake")));
        assert_eq!(show(&names, Kind::Other, "Église", Some("eglise"), &[], FR), ds("Church", None));
        // Nowhere (the high seas): OSM's languages only.
        assert_eq!(show(&names, Kind::Other, "Lac Bleu", None, &[], &[]), ds("Lac Bleu", None));
        assert_eq!(show(&names, Kind::Other, "Lac Bleu", None, &["fr"], &[]), ds("Lac Bleu", Some("Blue Lake")));
        assert_eq!(names.translation(Kind::Other, "中山", &ls(&["en", "zh"])).map(|t| t.lang), Some(l("zh")));
        assert!(names.has_line(Kind::Road, "中山", &ls(JP)));
        assert!(!names.has_line(Kind::Road, "中山", &ls(TW)));
    }

    #[test]
    fn later_files_win_and_versions_are_per_language() {
        let d = Dir::new();
        d.write("0-converted/french.jsonl", &(line("Lac Noir", "\"other\"", "[\"fr\"]", None, Some("Lake Noir")) + &line("Rue Haute", "\"road\"", "[\"fr\"]", None, None)));
        d.write("0-converted/japanese.jsonl", &line("松島", "\"other\"", "[\"ja\"]", None, Some("Matsu-shima")));
        let names = Names::load(&d.0).expect("load");
        let (fr1, ja1) = (names.version(l("fr")), names.version(l("ja")));
        assert_eq!(show(&names, Kind::Other, "Lac Noir", None, &[], FR), ds("Lac Noir", Some("Lake Noir")));
        d.write("answers/2026-10-09.jsonl", &line("Lac Noir", "\"other\"", "[\"fr\"]", None, Some("Black Lake")));
        let mut names2 = names.clone();
        // A new file, read once it has held for ten seconds.
        let t0 = Instant::now();
        assert!(!names2.scan(t0, SystemTime::now()).expect("scan"));
        assert!(names2.scan(t0 + Duration::from_secs(11), SystemTime::now()).expect("scan"));
        assert_eq!(show(&names2, Kind::Other, "Lac Noir", None, &[], FR), ds("Lac Noir", Some("Black Lake")));
        assert_eq!(show(&names2, Kind::Road, "Rue Haute", None, &[], FR), ds("Rue Haute", None));
        // A French drop changes French's version, not Japanese's.
        assert_ne!(names2.version(l("fr")), fr1);
        assert_eq!(names2.version(l("ja")), ja1);
        assert_eq!(names2.version_of(&ls(&["ja"])), names.version_of(&ls(&["ja"])));
        assert_ne!(names2.version_of(&ls(&["fr", "ja"])), names.version_of(&ls(&["fr", "ja"])));
        assert_ne!(names2.version_all(), names.version_all());
        // The clone kept its tables.
        assert_eq!(show(&names, Kind::Other, "Lac Noir", None, &[], FR), ds("Lac Noir", Some("Lake Noir")));
        let s = names2.summary();
        assert_eq!((s.files, s.lines, s.old), (3, 4, 0));
        assert_eq!(s.langs.keys().cloned().collect::<Vec<_>>(), ["fr", "ja"]);
    }

    #[test]
    fn the_area_tables_lines_are_left_out_and_said() {
        let d = Dir::new();
        d.write("fr/places-fr.jsonl", "{\"n\": \"Château\", \"main\": \"Castle\", \"sub\": null, \"via\": \"rule:bare\"}\n{\"n\": \"Lac\", \"en\": \"Lake\"}\n");
        d.write("todo/french.jsonl", &line("Château", "\"other\"", "[\"fr\"]", Some("Wrong"), None));
        d.write(".hidden.jsonl", &line("Château", "\"other\"", "[\"fr\"]", Some("Wrong"), None));
        d.write("fr/readme.txt", "not a table");
        let mut names = Names::load(&d.0).expect("load");
        let s = names.summary();
        assert_eq!((s.files, s.lines, s.old), (0, 0, 2));
        assert!(s.only_old());
        let w = names.take_warnings();
        assert_eq!(w, ["2 lines of the area tables' format (no kind or languages) left out, in 1 files (fr/places-fr.jsonl)"]);
        assert!(names.tables.is_empty());
        assert_eq!(show(&names, Kind::Other, "Château", None, &[], FR), ds("Château", None));
        d.write("0-converted/french.jsonl", &line("Château", "\"other\"", "[\"fr\"]", Some("Castle"), None));
        let names = Names::load(&d.0).expect("load");
        assert!(!names.summary().only_old());
        assert_eq!(show(&names, Kind::Other, "Château", None, &[], FR), ds("Castle", None));
        assert!(names.heap_bytes() > 0);
        assert_eq!(names.pending(), None);
    }

    #[test]
    fn kinds_of_places() {
        assert_eq!(Kind::of_place("village"), Kind::Settlement);
        assert_eq!(Kind::of_place("isolated_dwelling"), Kind::Settlement);
        assert_eq!(Kind::of_place("island"), Kind::Other);
        assert_eq!(Kind::of_place("locality"), Kind::Other);
        assert_eq!(Kind::parse("road"), Some(Kind::Road));
        assert_eq!(Kind::parse(Kind::Settlement.as_str()), Some(Kind::Settlement));
        assert_eq!(Kind::parse("place"), None);
    }

    #[test]
    fn warnings_for_bad_lines() {
        let d = Dir::new();
        let a = line("A", "\"other\"", "\"fr\"", None, Some("a"));
        d.write("x.jsonl", &format!("{a}nope\n{{\"n\": \"B\", \"kind\": \"other\", \"langs\": \"fr\"}}\n{}{{\"n\": \"D\", \"sub", line("C", "\"other\"", "\"fr\"", None, Some("c"))));
        let mut names = Names::load(&d.0).expect("load");
        let w = names.take_warnings();
        assert_eq!(w, vec!["x.jsonl: 2 malformed lines skipped (first: line 2)".to_owned(), "x.jsonl: unfinished last line ignored".to_owned()]);
        assert_eq!(names.entries(), 2);
        assert!(names.take_warnings().is_empty());
    }

    #[test]
    fn refresh_waits_for_files_to_settle() {
        let d = Dir::new();
        d.write("jp/a.jsonl", "{\"n\": \"松島\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\": \"Matsushima\"}\n");
        let t0 = Instant::now();
        let wall = SystemTime::now();
        let mut names = Names::new(&d.0);
        assert!(names.scan(t0, wall).expect("scan"));
        let v1 = names.version(l("ja"));
        assert_ne!(v1, names.version(l("zh")));
        assert_eq!(names.version(l("zh")), Names::new(&d.0).version(l("zh")));

        // A new file, just written: not read until it has held for ten seconds.
        d.write_aged("jp/b.jsonl", "{\"n\": \"松島\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\": \"Matsu-shima\"}\n", Duration::ZERO);
        assert!(!names.scan(t0 + Duration::from_secs(1), wall).expect("scan"));
        assert!(names.pending().is_some());
        assert_eq!(names.translation(Kind::Other, "松島", &[l("ja")]).and_then(|t| t.sub), Some("Matsushima"));
        assert!(!names.scan(t0 + Duration::from_secs(10), wall).expect("scan"));
        assert!(names.scan(t0 + Duration::from_secs(11), wall).expect("scan"));
        assert_eq!(names.translation(Kind::Other, "松島", &[l("ja")]).and_then(|t| t.sub), Some("Matsu-shima"));
        assert_eq!(names.pending(), None);
        let v2 = names.version(l("ja"));
        assert_ne!(v1, v2);

        // Unchanged: nothing read, the version holds.
        assert!(!names.scan(t0 + Duration::from_secs(30), wall).expect("scan"));
        assert_eq!(names.version(l("ja")), v2);

        // Still growing: each change restarts the wait, and the old table stays meanwhile.
        d.write_aged("jp/b.jsonl", "{\"n\": \"松島\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\": \"Matsu-shima\"}\n{\"n\": \"中山\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\":", Duration::ZERO);
        assert!(!names.scan(t0 + Duration::from_secs(40), wall).expect("scan"));
        d.write_aged("jp/b.jsonl", "{\"n\": \"松島\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\": \"Matsu-shima\"}\n{\"n\": \"中山\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\": \"Nakayama\"}\n", Duration::ZERO);
        assert!(!names.scan(t0 + Duration::from_secs(45), wall).expect("scan"));
        assert_eq!(names.translation(Kind::Other, "中山", &[l("ja")]), None);
        assert!(!names.scan(t0 + Duration::from_secs(54), wall).expect("scan"));
        assert!(names.scan(t0 + Duration::from_secs(56), wall).expect("scan"));
        assert_eq!(names.translation(Kind::Other, "中山", &[l("ja")]).and_then(|t| t.sub), Some("Nakayama"));
        let v3 = names.version(l("ja"));
        assert_ne!(v2, v3);

        // Removed: its entries go at once.
        fs::remove_file(d.0.join("jp/b.jsonl")).expect("rm");
        assert!(names.scan(t0 + Duration::from_secs(60), wall).expect("scan"));
        assert_eq!(names.translation(Kind::Other, "中山", &[l("ja")]), None);
        assert_eq!(names.translation(Kind::Other, "松島", &[l("ja")]).and_then(|t| t.sub), Some("Matsushima"));
        assert_eq!(names.version(l("ja")), v1);

        // The folder gone (the NAS away): an error, and the tables stay.
        let kept = d.0.with_extension("away");
        fs::rename(&d.0, &kept).expect("mv");
        assert!(names.scan(t0 + Duration::from_secs(70), wall).is_err());
        assert_eq!(names.translation(Kind::Other, "松島", &[l("ja")]).and_then(|t| t.sub), Some("Matsushima"));
        fs::rename(&kept, &d.0).expect("mv back");
        assert!(!names.scan(t0 + Duration::from_secs(80), wall).expect("scan"));
    }

    #[test]
    fn first_scan_trusts_old_files_only() {
        // (Its files' times set from the scan's wall clock: an hour before it, and 2 s, however long
        // a busy Mac takes between writing them and scanning.)
        let d = Dir::new();
        let wall = SystemTime::now();
        d.write_at("jp/old.jsonl", "{\"n\": \"A\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\": \"a\"}\n", wall - Duration::from_secs(3600));
        d.write_at("jp/new.jsonl", "{\"n\": \"B\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\": \"b\"}\n", wall - Duration::from_secs(2));
        let t0 = Instant::now();
        let mut names = Names::new(&d.0);
        assert!(names.scan(t0, wall).expect("scan"));
        assert!(names.translation(Kind::Other, "A", &[l("ja")]).is_some());
        assert!(names.translation(Kind::Other, "B", &[l("ja")]).is_none());
        assert!(names.pending().is_some());
        assert!(names.scan(t0 + Duration::from_secs(10), wall + Duration::from_secs(10)).expect("scan"));
        assert!(names.translation(Kind::Other, "B", &[l("ja")]).is_some());
    }

    #[test]
    fn unreadable_files_are_retried_quietly() {
        use std::os::unix::fs::PermissionsExt;
        let d = Dir::new();
        d.write("jp/a.jsonl", "{\"n\": \"A\", \"kind\": \"other\", \"langs\": \"ja\", \"sub\": \"a\"}\n");
        let p = d.0.join("jp/a.jsonl");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o000)).expect("chmod");
        if File::open(&p).is_ok() {
            eprintln!("can read a mode 000 file (root?): skipped");
            return;
        }
        let mut names = Names::load(&d.0).expect("load");
        let w = names.take_warnings();
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].starts_with("jp/a.jsonl: opening: "), "{w:?}");
        assert_eq!(names.pending(), None);
        assert!(!names.refresh().expect("refresh"));
        assert!(names.take_warnings().is_empty());
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(names.refresh().expect("refresh"));
        assert_eq!(names.translation(Kind::Other, "A", &[l("ja")]).and_then(|t| t.sub), Some("a"));
    }
}
