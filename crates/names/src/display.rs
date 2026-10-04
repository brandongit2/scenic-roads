//! The display rule (docs/plan.md §7) and the translation tables it reads.
//!
//! A name shows as a **main** label and an optional smaller **sub** line: the translation line for
//! the name in its reading area when there is one, else the name with the thing's own English as
//! sub. Sub is dropped when it is empty, already one of main's parts ("Alba / Scotland" and
//! "Scotland"), or the same name but for accents, case, punctuation or spacing (Montréal, Montreal).
//!
//! Places and roads keep separate tables ([`Kind`]), as the translation work writes them
//! (`places-jp.jsonl`, `roads-jp.jsonl`): the same words can be a place to translate and a road to
//! keep ("Château" is a castle, and a road in France is named Château). A name is looked up in its
//! kind's table first, then in the other's. Lines the translation work hasn't done yet (`via`
//! "todo" or "skipped") are left out, so they don't hide the thing's own English.
//!
//! The tables are read from a translations folder (the server's local copy of `translations/` on
//! the NAS, `translations/**/*.jsonl`). [`Names::refresh`] picks up dropped, replaced and removed files, reading a file only once
//! its size and modification time have held for ten seconds, so a file still being copied is never
//! read. Lookups never touch the disk: refresh a clone (cheap, the tables are shared) and swap it
//! in, so requests never wait on the NAS.

use crate::area::{area_at, is_area};
use crate::table::{self, Report, Table};
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

/// Hashed into every area's version: bump it when the display rule or the reading of the files
/// changes, so ETags made under the old rule stop matching. (2: tables by kind, lines not
/// translated yet left out.)
const RULES: &[u8] = b"names display rule 2";

/// Which table a name is read in first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Everything named on the map but roads: places, water, parks, peaks, stations and ferry
    /// terminals, rail and ferry lines, sights and heritage sites (the translation work's
    /// `places-<area>` files hold all of these).
    Place,
    /// Road names (`roads-<area>`).
    Road,
}

impl Kind {
    fn other(self) -> Kind {
        match self {
            Kind::Place => Kind::Road,
            Kind::Road => Kind::Place,
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

/// A name's translation line, as written (before the sub rule).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Translation<'a> {
    pub main: &'a str,
    pub sub: Option<&'a str>,
}

/// One area's table, for status and ETags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AreaSummary<'a> {
    pub code: &'a str,
    /// [`Names::version`].
    pub version: u64,
    pub files: usize,
    /// Distinct names per file, summed.
    pub entries: usize,
    /// Lines left out as not translated yet (`via` "todo" or "skipped").
    pub ignored: usize,
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

/// A `.jsonl` file under the folder, as last seen.
#[derive(Clone, Debug)]
struct Tracked {
    /// Its area, or why it has none (skipped).
    area: Result<String, String>,
    /// The kind of names it holds (`None`: both).
    kind: Option<Kind>,
    /// Size and modification time when last seen, and since when (by the monotonic clock).
    seen: Stat,
    since: Instant,
    /// Seen at the first scan already ten seconds old by its own modification time.
    aged: bool,
    /// Reading it (as `seen`) failed: tried again at each scan, warned about once.
    failed: bool,
    /// The table in use and the stat it was read at.
    loaded: Option<(Stat, Arc<Table>)>,
}

#[derive(Clone, Debug, Default)]
struct Area {
    /// By relative path (later files win), with the kind each holds (`None`: both).
    tables: Vec<(Option<Kind>, Arc<Table>)>,
    version: u64,
}

/// The translation tables of a translations folder, by area.
#[derive(Clone)]
pub struct Names {
    dir: PathBuf,
    stable: Duration,
    scanned: bool,
    files: BTreeMap<String, Tracked>,
    areas: BTreeMap<String, Area>,
    warnings: Vec<String>,
}

impl fmt::Debug for Names {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Names")
            .field("dir", &self.dir)
            .field("areas", &self.areas().collect::<Vec<_>>())
            .field("pending", &self.pending())
            .finish()
    }
}

/// What reading one file came to.
enum Outcome {
    Read(Table, Report),
    /// It changed while being read: wait for it to hold again.
    Moved(Stat),
    Gone,
    Failed(anyhow::Error),
}

impl Names {
    /// Tables for `dir` with nothing read yet: the first [`refresh`](Self::refresh) reads what is
    /// there as [`load`](Self::load) would. For starting while the NAS is away.
    pub fn new(dir: &Path) -> Names {
        Names {
            dir: dir.to_owned(),
            stable: STABLE,
            scanned: false,
            files: BTreeMap::new(),
            areas: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Reads every `*.jsonl` file under `dir`, skipping hidden files and folders and `todo/`.
    ///
    /// A file's area is its first folder under `dir` when that is a lowercase ASCII word
    /// (`jp/places-jp.jsonl`, or any new code), else the area code in its file name, split on `-`,
    /// `_` and `.` (`places-jp.out.jsonl`); files with neither are skipped with a warning
    /// ([`take_warnings`](Self::take_warnings)). A file holds places or roads ([`Kind`]) by a
    /// `places` (or `place`) or `roads` (or `road`) word in its path, split as above; with neither
    /// (or both) it holds both. Within an area and kind, files later by relative path win for the
    /// same name.
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
    /// whether any table changed (and with it some area's [`version`](Self::version)). Fails, with
    /// nothing changed, when the folder can't be listed.
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
            let fresh = match self.files.get_mut(&rel) {
                Some(t) if t.seen == stat => false,
                Some(t) => {
                    (t.seen, t.since, t.aged, t.failed) = (stat, now, false, false);
                    true
                }
                None => {
                    let t = Tracked {
                        area: area_of(&rel),
                        kind: kind_of(&rel),
                        seen: stat,
                        since: now,
                        aged: first && stat.age(wall) >= self.stable,
                        failed: false,
                        loaded: None,
                    };
                    self.files.insert(rel.clone(), t);
                    true
                }
            };
            let Some(t) = self.files.get(&rel) else { continue };
            if let Err(why) = &t.area {
                if fresh {
                    self.warnings.push(format!("{rel}: skipped, {why}"));
                }
                continue;
            }
            let current = t.loaded.as_ref().is_some_and(|(s, _)| *s == stat);
            let settled = t.aged || now.saturating_duration_since(t.since) >= self.stable;
            if !current && settled {
                jobs.push((rel, path, stat));
            }
        }

        let outcomes = read_all(&jobs);
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
                    if let Some(t) = self.files.get_mut(&rel) {
                        t.loaded = Some((stat, Arc::new(table)));
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
        if changed {
            self.rebuild();
        }
        Ok(changed)
    }

    fn rebuild(&mut self) {
        let mut areas: BTreeMap<String, Area> = BTreeMap::new();
        let mut keys: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (rel, t) in &self.files {
            let (Ok(code), Some((stat, table))) = (&t.area, &t.loaded) else { continue };
            areas.entry(code.clone()).or_default().tables.push((t.kind, table.clone()));
            let key = keys.entry(code.clone()).or_insert_with(|| RULES.to_vec());
            key.extend_from_slice(rel.as_bytes());
            key.push(0);
            key.extend_from_slice(&stat.size.to_le_bytes());
            key.extend_from_slice(&stat.mtime.to_le_bytes());
        }
        for (code, a) in &mut areas {
            a.version = keys.get(code).map_or(0, |k| table::hash(k));
        }
        self.areas = areas;
    }

    /// The folder read.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A hash of `area`'s table: of its files' relative paths, sizes and modification times (and
    /// the display rule). It changes whenever the table does and holds otherwise, across runs too:
    /// for HTTP ETags of anything showing names read in that area.
    pub fn version(&self, area: &str) -> u64 {
        self.areas.get(area).map_or_else(|| table::hash(RULES), |a| a.version)
    }

    /// The areas with tables, by code.
    pub fn areas(&self) -> impl Iterator<Item = AreaSummary<'_>> {
        self.areas.iter().map(|(code, a)| AreaSummary {
            code,
            version: a.version,
            files: a.tables.len(),
            entries: a.tables.iter().map(|(_, t)| t.len()).sum(),
            ignored: a.tables.iter().map(|(_, t)| t.ignored()).sum(),
        })
    }

    /// Distinct names per file, summed over every table.
    pub fn entries(&self) -> usize {
        self.areas.values().flat_map(|a| &a.tables).map(|(_, t)| t.len()).sum()
    }

    /// Heap bytes held by the tables.
    pub fn heap_bytes(&self) -> usize {
        self.areas.values().flat_map(|a| &a.tables).map(|(_, t)| t.heap_bytes()).sum()
    }

    /// When new or changed files are waiting to settle: how long until the first may be read (zero:
    /// [`refresh`](Self::refresh) now), so a caller polling once a minute can look again sooner.
    /// Files that failed to read aren't waiting: each refresh tries them again.
    pub fn pending(&self) -> Option<Duration> {
        let now = Instant::now();
        self.files
            .values()
            .filter(|t| t.area.is_ok() && !t.failed && !t.loaded.as_ref().is_some_and(|(s, _)| *s == t.seen))
            .map(|t| if t.aged { Duration::ZERO } else { self.stable.saturating_sub(now.saturating_duration_since(t.since)) })
            .min()
    }

    /// The warnings since the last call: files skipped (no area), malformed lines, unfinished last
    /// lines, files that failed to read.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// The translation line for a `kind` of name in `area`, as written: from that kind's files
    /// (and those holding both) when they have one, else from the other kind's; the latest file's
    /// within each.
    pub fn translation<'a>(&'a self, kind: Kind, area: &str, name: &'a str) -> Option<Translation<'a>> {
        let a = self.areas.get(area)?;
        let h = table::hash(name.as_bytes());
        // First the files of this kind (or of both), then the other kind's.
        let first = |k: Option<Kind>| k != Some(kind.other());
        let find = |in_first: bool| a.tables.iter().rev().filter(|(k, _)| first(*k) == in_first).find_map(|(_, t)| t.get(h, name.as_bytes()));
        let r = find(true).or_else(|| find(false))?;
        Some(Translation { main: r.main.unwrap_or(name), sub: r.sub })
    }

    /// How a `kind` of name shows at `lon`, `lat`, given the thing's own English (OSM's `name:en`,
    /// …).
    pub fn display(&self, kind: Kind, name: &str, own_en: Option<&str>, lon: f64, lat: f64) -> Display {
        self.display_ref(kind, name, own_en, lon, lat).to_display()
    }

    /// [`display`](Self::display), borrowed.
    pub fn display_ref<'a>(&'a self, kind: Kind, name: &'a str, own_en: Option<&'a str>, lon: f64, lat: f64) -> DisplayRef<'a> {
        self.display_in(kind, area_at(lon, lat), name, own_en)
    }

    /// How a `kind` of name shows when read in `area` (`None`: outside every area, own English
    /// only).
    pub fn display_in<'a>(&'a self, kind: Kind, area: Option<&str>, name: &'a str, own_en: Option<&'a str>) -> DisplayRef<'a> {
        match area.and_then(|a| self.translation(kind, a, name)) {
            Some(t) => DisplayRef::new(t.main, t.sub),
            None => DisplayRef::new(name, own_en),
        }
    }
}

/// A file's area from its path under the folder, or why it has none.
fn area_of(rel: &str) -> Result<String, String> {
    let parts: Vec<&str> = rel.split('/').collect();
    if let [folder, _, ..] = parts.as_slice() {
        if !folder.is_empty() && folder.bytes().all(|b| b.is_ascii_lowercase()) {
            return Ok((*folder).to_owned());
        }
    }
    let file = parts.last().copied().unwrap_or_default();
    let mut codes: Vec<&str> = file.split(['-', '_', '.']).filter(|t| is_area(t)).collect();
    codes.dedup();
    match codes.as_slice() {
        [code] => Ok((*code).to_owned()),
        [] => Err("no area (put it in an area's folder, or name the area in the file name: places-jp.jsonl)".to_owned()),
        _ => Err(format!("several areas in its name ({})", codes.join(", "))),
    }
}

/// The kind of names a file holds, from the words of its path (`None`: both).
fn kind_of(rel: &str) -> Option<Kind> {
    let words = || rel.split(['/', '-', '_', '.']);
    let places = words().any(|w| w == "places" || w == "place");
    let roads = words().any(|w| w == "roads" || w == "road");
    match (places, roads) {
        (true, false) => Some(Kind::Place),
        (false, true) => Some(Kind::Road),
        _ => None,
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

    const TOKYO: (f64, f64) = (139.69, 35.69);
    const PARIS: (f64, f64) = (2.35, 48.86);

    /// How a place name shows.
    fn show(names: &Names, name: &str, own: Option<&str>, at: (f64, f64)) -> (String, Option<String>) {
        show_as(names, Kind::Place, name, own, at)
    }

    fn show_as(names: &Names, kind: Kind, name: &str, own: Option<&str>, at: (f64, f64)) -> (String, Option<String>) {
        let d = names.display(kind, name, own, at.0, at.1);
        (d.main, d.sub)
    }

    fn ds(main: &str, sub: Option<&str>) -> (String, Option<String>) {
        (main.to_owned(), sub.map(str::to_owned))
    }

    #[test]
    fn areas_from_paths() {
        assert_eq!(area_of("jp/places-jp.jsonl"), Ok("jp".to_owned()));
        assert_eq!(area_of("jp/2026/x.jsonl"), Ok("jp".to_owned()));
        assert_eq!(area_of("de/strassen.jsonl"), Ok("de".to_owned())); // any new code as a folder
        assert_eq!(area_of("jp/roads-tw.jsonl"), Ok("jp".to_owned())); // the folder decides
        assert_eq!(area_of("places-jp.out.jsonl"), Ok("jp".to_owned()));
        assert_eq!(area_of("roads_tw.jsonl"), Ok("tw".to_owned()));
        assert_eq!(area_of("Archive 2026/places-gb.jsonl"), Ok("gb".to_owned()));
        assert_eq!(area_of("JP/places.jsonl").map_err(|_| ()), Err(()));
        assert_eq!(area_of("places.jsonl").map_err(|_| ()), Err(()));
        assert_eq!(area_of("japan.jsonl").map_err(|_| ()), Err(()));
        assert_eq!(area_of("jp-tw.jsonl").map_err(|_| ()), Err(()));
        assert_eq!(area_of("jp-places-jp.jsonl"), Ok("jp".to_owned()));
    }

    #[test]
    fn kinds_from_paths() {
        assert_eq!(kind_of("jp/places-jp.jsonl"), Some(Kind::Place));
        assert_eq!(kind_of("jp/roads-jp.jsonl"), Some(Kind::Road));
        assert_eq!(kind_of("places-fr-1.jsonl"), Some(Kind::Place));
        assert_eq!(kind_of("roads_tw.out.jsonl"), Some(Kind::Road));
        assert_eq!(kind_of("jp/roads/2026-10.jsonl"), Some(Kind::Road));
        assert_eq!(kind_of("jp/place-fixes.jsonl"), Some(Kind::Place));
        assert_eq!(kind_of("jp/fixes.jsonl"), None);
        assert_eq!(kind_of("jp/placeholder.jsonl"), None);
        assert_eq!(kind_of("places-and-roads.jsonl"), None);
    }

    #[test]
    fn places_and_roads_have_their_own_tables() {
        let d = Dir::new();
        d.write("fr/places-fr.jsonl", "{\"n\": \"Château\", \"main\": \"Castle\", \"sub\": null}\n{\"n\": \"Lac Noir\", \"main\": \"Lac Noir\", \"sub\": \"Black Lake\"}\n");
        d.write("fr/roads-fr.jsonl", "{\"n\": \"Château\", \"main\": \"Château\", \"sub\": null}\n{\"n\": \"Avenue Foch\", \"main\": \"Avenue Foch\", \"sub\": \"Foch Avenue\"}\n");
        let names = Names::load(&d.0).expect("load");
        // Each kind its own line.
        assert_eq!(show(&names, "Château", None, PARIS), ds("Castle", None));
        assert_eq!(show_as(&names, Kind::Road, "Château", None, PARIS), ds("Château", None));
        // A name only the other kind's file has: that line.
        assert_eq!(show(&names, "Avenue Foch", None, PARIS), ds("Avenue Foch", Some("Foch Avenue")));
        assert_eq!(show_as(&names, Kind::Road, "Lac Noir", None, PARIS), ds("Lac Noir", Some("Black Lake")));
        assert_eq!(show_as(&names, Kind::Road, "Rue Haute", Some("High Street"), PARIS), ds("Rue Haute", Some("High Street")));

        // A file of both kinds is read with each kind's own files, by path.
        d.write("fr/zz-fixes.jsonl", "{\"n\": \"Château\", \"main\": \"Castle\", \"sub\": \"Château\"}\n{\"n\": \"Gué\", \"en\": \"Ford\"}\n");
        d.write("fr/aa-old.jsonl", "{\"n\": \"Lac Noir\", \"en\": \"Lake Noir\"}\n");
        let names = Names::load(&d.0).expect("load");
        assert_eq!(show(&names, "Château", None, PARIS), ds("Castle", Some("Château")));
        assert_eq!(show_as(&names, Kind::Road, "Château", None, PARIS), ds("Castle", Some("Château")));
        assert_eq!(show_as(&names, Kind::Road, "Gué", None, PARIS), ds("Gué", Some("Ford")));
        // aa-old sorts before places-fr: the places line wins for places, and is the roads'
        // fallback only after aa-old, which roads read first.
        assert_eq!(show(&names, "Lac Noir", None, PARIS), ds("Lac Noir", Some("Black Lake")));
        assert_eq!(show_as(&names, Kind::Road, "Lac Noir", None, PARIS), ds("Lac Noir", Some("Lake Noir")));
    }

    #[test]
    fn lines_not_translated_yet_are_left_out() {
        let d = Dir::new();
        d.write("fr/places-fr.jsonl", "{\"n\": \"Pont Vieux\", \"main\": \"Old Bridge\", \"sub\": null}\n{\"n\": \"Lac Bleu\", \"main\": \"Lac Bleu\", \"sub\": null, \"via\": \"todo\"}\n");
        d.write("fr/zz.jsonl", "{\"n\": \"Pont Vieux\", \"main\": \"Pont Vieux\", \"sub\": null, \"via\": \"skipped\"}\n");
        d.write("fr/roads-fr.jsonl", "{\"n\": \"Lac Bleu\", \"main\": \"Lac Bleu\", \"sub\": null, \"via\": \"skipped\"}\n");
        let names = Names::load(&d.0).expect("load");
        // The thing's own English shows; a later file's mark hides no earlier translation.
        assert_eq!(show(&names, "Lac Bleu", Some("Blue Lake"), PARIS), ds("Lac Bleu", Some("Blue Lake")));
        assert_eq!(show(&names, "Pont Vieux", Some("Old Bridge of Nowhere"), PARIS), ds("Old Bridge", None));
        assert_eq!(names.translation(Kind::Road, "fr", "Lac Bleu"), None);
        assert_eq!(names.areas().map(|a| (a.code, a.entries, a.ignored)).collect::<Vec<_>>(), vec![("fr", 1, 3)]);
    }

    #[test]
    fn loads_and_displays() {
        let d = Dir::new();
        d.write("jp/places-jp.jsonl", "{\"n\": \"松島\", \"main\": \"松島\", \"sub\": \"Matsu-shima\", \"case\": \"7\", \"via\": \"rule:roman\"}\n{\"n\": \"中山\", \"main\": \"中山\", \"sub\": \"Nakayama\"}\n");
        d.write("jp/roads-jp.jsonl", "{\"n\": \"中山\", \"main\": \"中山\", \"sub\": \"Naka-yama\"}\n");
        d.write("places-tw.out.jsonl", "{\"n\": \"中山\", \"en\": \"Zhongshan\"}\n");
        d.write("fr/places-fr.jsonl", "{\"n\": \"Église\", \"main\": \"Church\", \"sub\": null}\n{\"n\": \"Rue Principale\", \"main\": \"Rue Principale\", \"sub\": null}\n");
        d.write("todo/jp.jsonl", "{\"n\": \"松島\", \"en\": \"Wrong\"}\n");
        d.write(".hidden.jsonl", "{\"n\": \"松島\", \"en\": \"Wrong\"}\n");
        d.write("jp/._places-jp.jsonl", "junk");
        d.write("notes.jsonl", "{\"n\": \"松島\", \"en\": \"Wrong\"}\n");
        d.write("jp/readme.txt", "not a table");
        let mut names = Names::load(&d.0).expect("load");
        let warnings = names.take_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("notes.jsonl: skipped, no area"), "{warnings:?}");
        assert_eq!(names.entries(), 6);
        assert_eq!(names.areas().map(|a| (a.code, a.files, a.entries)).collect::<Vec<_>>(), vec![("fr", 1, 2), ("jp", 2, 3), ("tw", 1, 1)]);

        assert_eq!(show(&names, "松島", None, TOKYO), ds("松島", Some("Matsu-shima")));
        // A place reads the places file, a road the roads file; Taiwan reads the name its own way.
        assert_eq!(show(&names, "中山", None, TOKYO), ds("中山", Some("Nakayama")));
        assert_eq!(show_as(&names, Kind::Road, "中山", None, TOKYO), ds("中山", Some("Naka-yama")));
        assert_eq!(show(&names, "中山", None, (121.56, 25.04)), ds("中山", Some("Zhongshan")));
        assert_eq!(show_as(&names, Kind::Road, "中山", None, (121.56, 25.04)), ds("中山", Some("Zhongshan")));
        // Outside every area: the thing's own English only.
        assert_eq!(show(&names, "中山", Some("Zhongshan Park"), (116.4, 39.9)), ds("中山", Some("Zhongshan Park")));
        assert_eq!(show(&names, "中山", None, (116.4, 39.9)), ds("中山", None));
        // A translation as main; a line with no sub overrides the thing's own English.
        assert_eq!(show(&names, "Église", Some("St Mary's"), PARIS), ds("Church", None));
        assert_eq!(show(&names, "Rue Principale", Some("Main Street"), PARIS), ds("Rue Principale", None));
        // No line: own English, by the sub rule.
        assert_eq!(show(&names, "Lac Noir", Some("Black Lake"), PARIS), ds("Lac Noir", Some("Black Lake")));
        assert_eq!(show(&names, "Montréal", Some("Montreal"), PARIS), ds("Montréal", None));
        assert_eq!(names.translation(Kind::Place, "jp", "中山"), Some(Translation { main: "中山", sub: Some("Nakayama") }));
        assert_eq!(names.translation(Kind::Road, "jp", "中山"), Some(Translation { main: "中山", sub: Some("Naka-yama") }));
        assert_eq!(names.translation(Kind::Place, "gb", "中山"), None);
        assert!(names.heap_bytes() > 0);
        assert_eq!(names.pending(), None);
    }

    #[test]
    fn refresh_waits_for_files_to_settle() {
        let d = Dir::new();
        d.write("jp/a.jsonl", "{\"n\": \"松島\", \"en\": \"Matsushima\"}\n");
        let t0 = Instant::now();
        let wall = SystemTime::now();
        let mut names = Names::new(&d.0);
        assert!(names.scan(t0, wall).expect("scan"));
        let v1 = names.version("jp");
        assert_ne!(v1, names.version("tw"));
        assert_eq!(names.version("tw"), Names::new(&d.0).version("tw"));

        // A new file, just written: not read until it has held for ten seconds.
        d.write_aged("jp/b.jsonl", "{\"n\": \"松島\", \"en\": \"Matsu-shima\"}\n", Duration::ZERO);
        assert!(!names.scan(t0 + Duration::from_secs(1), wall).expect("scan"));
        assert!(names.pending().is_some());
        assert_eq!(names.translation(Kind::Place, "jp", "松島").and_then(|t| t.sub), Some("Matsushima"));
        assert!(!names.scan(t0 + Duration::from_secs(10), wall).expect("scan"));
        assert!(names.scan(t0 + Duration::from_secs(11), wall).expect("scan"));
        assert_eq!(names.translation(Kind::Place, "jp", "松島").and_then(|t| t.sub), Some("Matsu-shima"));
        assert_eq!(names.pending(), None);
        let v2 = names.version("jp");
        assert_ne!(v1, v2);

        // Unchanged: nothing read, the version holds.
        assert!(!names.scan(t0 + Duration::from_secs(30), wall).expect("scan"));
        assert_eq!(names.version("jp"), v2);

        // Still growing: each change restarts the wait, and the old table stays meanwhile.
        d.write_aged("jp/b.jsonl", "{\"n\": \"松島\", \"en\": \"Matsu-shima\"}\n{\"n\": \"中山\", \"en\":", Duration::ZERO);
        assert!(!names.scan(t0 + Duration::from_secs(40), wall).expect("scan"));
        d.write_aged("jp/b.jsonl", "{\"n\": \"松島\", \"en\": \"Matsu-shima\"}\n{\"n\": \"中山\", \"en\": \"Nakayama\"}\n", Duration::ZERO);
        assert!(!names.scan(t0 + Duration::from_secs(45), wall).expect("scan"));
        assert_eq!(names.translation(Kind::Place, "jp", "中山"), None);
        assert!(!names.scan(t0 + Duration::from_secs(54), wall).expect("scan"));
        assert!(names.scan(t0 + Duration::from_secs(56), wall).expect("scan"));
        assert_eq!(names.translation(Kind::Place, "jp", "中山").and_then(|t| t.sub), Some("Nakayama"));
        let v3 = names.version("jp");
        assert_ne!(v2, v3);

        // Removed: its entries go at once.
        fs::remove_file(d.0.join("jp/b.jsonl")).expect("rm");
        assert!(names.scan(t0 + Duration::from_secs(60), wall).expect("scan"));
        assert_eq!(names.translation(Kind::Place, "jp", "中山"), None);
        assert_eq!(names.translation(Kind::Place, "jp", "松島").and_then(|t| t.sub), Some("Matsushima"));
        assert_eq!(names.version("jp"), v1);

        // The folder gone (the NAS away): an error, and the tables stay.
        let kept = d.0.with_extension("away");
        fs::rename(&d.0, &kept).expect("mv");
        assert!(names.scan(t0 + Duration::from_secs(70), wall).is_err());
        assert_eq!(names.translation(Kind::Place, "jp", "松島").and_then(|t| t.sub), Some("Matsushima"));
        fs::rename(&kept, &d.0).expect("mv back");
        assert!(!names.scan(t0 + Duration::from_secs(80), wall).expect("scan"));
    }

    #[test]
    fn first_scan_trusts_old_files_only() {
        let d = Dir::new();
        d.write("jp/old.jsonl", "{\"n\": \"A\", \"en\": \"a\"}\n");
        d.write_aged("jp/new.jsonl", "{\"n\": \"B\", \"en\": \"b\"}\n", Duration::from_secs(2));
        let t0 = Instant::now();
        let mut names = Names::new(&d.0);
        assert!(names.scan(t0, SystemTime::now()).expect("scan"));
        assert!(names.translation(Kind::Place, "jp", "A").is_some());
        assert!(names.translation(Kind::Place, "jp", "B").is_none());
        assert!(names.pending().is_some());
        assert!(names.scan(t0 + Duration::from_secs(10), SystemTime::now()).expect("scan"));
        assert!(names.translation(Kind::Place, "jp", "B").is_some());
    }

    #[test]
    fn unreadable_files_are_retried_quietly() {
        use std::os::unix::fs::PermissionsExt;
        let d = Dir::new();
        d.write("jp/a.jsonl", "{\"n\": \"A\", \"en\": \"a\"}\n");
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
        assert_eq!(names.translation(Kind::Place, "jp", "A").and_then(|t| t.sub), Some("a"));
    }

    #[test]
    fn warnings_for_bad_lines() {
        let d = Dir::new();
        d.write("gb/x.jsonl", "{\"n\": \"A\", \"en\": \"a\"}\nnope\n{\"n\": \"B\"}\n{\"n\": \"C\", \"en\": \"c\"}\n{\"n\": \"D\", \"en");
        let mut names = Names::load(&d.0).expect("load");
        let w = names.take_warnings();
        assert_eq!(w, vec!["gb/x.jsonl: 2 malformed lines skipped (first: line 2)".to_owned(), "gb/x.jsonl: unfinished last line ignored".to_owned()]);
        assert_eq!(names.entries(), 2);
        assert!(names.take_warnings().is_empty());
    }
}
