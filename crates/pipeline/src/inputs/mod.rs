//! The drop boxes and the checking gate (docs/inputs.md §3, §4). Every input the map is built from
//! has a folder on the NAS, its drop box (`inputs/<unit>/`), where the owner drops it in its shape.
//! A change is checked before it's taken in: the `inputs` step (crate::inputs::gate), one target per
//! gate unit, a job like any other (it hands off; the lead merges). A change with an unaccepted
//! finding is held, and the last accepted version stays in use. The build reads only the accepted
//! version: checked copies, content-named under `sources/inputs/<unit>/`, listed by an index whose
//! content name is the unit's version, named in the records as `sources/inputs/<unit>/@index` (`open`, the one
//! accessor: §7.1).
//!
//! The parts: the units and their checks (`Checks`; the test unit's, crate::inputs::gatetest); the
//! drop boxes' listing (`list`, `settle`), which the agent makes every two minutes off its loop
//! (crate::inputs::watch); the check job (crate::inputs::gate); the acceptances (`accept`,
//! `unaccept`); the status's entries (crate::inputs::view); the credits from the inputs'
//! descriptions (crate::inputs::credits).
//!
//! On the gate (docs/inputs.md §4.10): the languages (#136, crate::inputs::languages), the
//! translations and the descriptions (#137), the regions (#135), the timetable feeds (#140,
//! crate::inputs::feeds), the heritage registers and the owner's matches (#138,
//! crate::inputs::heritage, crate::inputs::heritage_matches), and the test unit `_gate-test` while
//! `state/inputs/gate-test` exists (`TEST_FLAG`). Each other input joins `UNITS` in its own task
//! (#139, #141–#145).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub mod credits;
pub mod descriptions;
pub mod feeds;
pub mod gate;
pub mod gatetest;
pub mod heritage;
pub mod heritage_matches;
pub mod languages;
pub mod lint;
pub mod regions;
pub mod served;
pub mod translations;
pub mod view;
pub mod watch;

/// The `inputs` step's version: bumping it checks every unit again.
pub const INPUTS_V: u32 = 1;

/// The gate units on the gate (each input's task adds its own).
pub const UNITS: [&str; 7] = [languages::UNIT, translations::UNIT, descriptions::UNIT, regions::UNIT, feeds::UNIT, heritage::UNIT, heritage_matches::UNIT];

/// The test unit (docs/inputs.md §4.10): not real data; on the gate while `TEST_FLAG` exists.
pub const TEST_UNIT: &str = "_gate-test";

/// A nested test unit, its name with a `/` as the spec's `timetables/gtfs` has: its checks are the
/// test unit's; never on the gate, the tests' alone.
pub const TEST_NESTED: &str = "_gate-nest/inner";

/// The control that puts the test unit on the gate (`scenic inputs test on|off`).
pub const TEST_FLAG: &str = "state/inputs/gate-test";

/// Where the owner's acceptances are kept, a folder per unit, a file per finding id.
pub const ACCEPTED: &str = "state/inputs/accepted";

/// The folders that hold the drop boxes (`inputs/`, and the request–fulfil inputs' folders until
/// they move under it, #137): nothing but the gate and `open` reads under them (§7.1).
pub const DROP_ROOTS: [&str; 3] = ["inputs", "translations", "descriptions"];

/// A file is taken only once its size and time have held this long (seconds): one being written
/// isn't checked half-written.
pub const QUIET_S: u64 = 10;

/// Whether `unit` is a test unit (shown only while it's on the gate).
pub fn is_test(unit: &str) -> bool {
    unit == TEST_UNIT || unit == TEST_NESTED
}

/// The units on the gate now: `UNITS`, and the test unit while its flag is there.
pub fn units(root: &Path) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = UNITS.to_vec();
    if root.join(TEST_FLAG).exists() {
        v.push(TEST_UNIT);
    }
    v
}

/// The regions' checks as a file's own checks and the listing read them (the check job's are
/// `Checks::prepared`'s, with the pass's outlines).
static REGIONS: std::sync::LazyLock<regions::Regions> = std::sync::LazyLock::new(regions::Regions::default);

/// The registers' checks as a file's own checks read them (the check job's are
/// `Checks::prepared`'s, with the accepted version's counts and the pass's outlines).
static HERITAGE: std::sync::LazyLock<heritage::Heritage> = std::sync::LazyLock::new(heritage::Heritage::default);
static MATCHES: std::sync::LazyLock<heritage_matches::Matches> = std::sync::LazyLock::new(heritage_matches::Matches::default);

/// The checks of unit `unit`; None for a unit this app doesn't know.
pub fn checks(unit: &str) -> Option<&'static dyn Checks> {
    match unit {
        TEST_UNIT => Some(&gatetest::GateTest(TEST_UNIT)),
        TEST_NESTED => Some(&gatetest::GateTest(TEST_NESTED)),
        languages::UNIT => Some(&languages::Languages { codes: None }),
        translations::UNIT => Some(&translations::Translations),
        descriptions::UNIT => Some(&descriptions::Descriptions),
        regions::UNIT => Some(&*REGIONS),
        feeds::UNIT => Some(&feeds::Feeds { catalogue_ids: None, territories: None }),
        heritage::UNIT => Some(&*HERITAGE),
        heritage_matches::UNIT => Some(&*MATCHES),
        _ => None,
    }
}

/// A unit's own records, in its folder under `sources/inputs/`: its accepted index, its held report,
/// its last listing. A `@` starts no component of a drop-box path (`ignored`) nor of a unit's name
/// (`valid_unit`), so these are never a copy's name nor a nested unit's (`a`'s copy of `b/x.json`
/// and unit `a/b`'s index can't be confused).
pub const RECORDS: [&str; 4] = ["@index", "@held", "@listed", "@switched"];

/// A unit's name: `/`-separated components of lower-case letters, digits, `-` and `_`
/// (`timetables/gtfs`).
pub fn valid_unit(u: &str) -> bool {
    !u.is_empty() && u.split('/').all(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'))
}

/// The records' entry naming unit `unit`'s accepted index (`sources/inputs/<unit>/@index`: a
/// content name is its logical name's, as the lead checks every entry's at merge).
pub fn logical(unit: &str) -> String {
    format!("{}/@index", store_dir(unit))
}

/// The records' entry naming unit `unit`'s report while a change is held.
pub fn held_logical(unit: &str) -> String {
    format!("{}/@held", store_dir(unit))
}

/// The records' entry recording that the build's readers read unit `unit`'s accepted version, for a
/// unit moved from a drop box read directly (the regions, §6.1.10): written once, by the unit's
/// check job, naming the migration's proof it was switched on.
pub fn switched_logical(unit: &str) -> String {
    format!("{}/@switched", store_dir(unit))
}

/// The records' entry naming the files' sizes and times unit `unit`'s last check listed (`Listed`:
/// the check's own state, apart from the index, so the version's name is its contents' alone).
pub fn listed_logical(unit: &str) -> String {
    format!("{}/@listed", store_dir(unit))
}

/// The unit of one of a unit's records' logical names (`sources/inputs/<unit>/@index`, `…/@held`,
/// `…/@listed`), and which (`@index`…); the unit's name may hold `/`s.
pub fn record_of(logical: &str) -> Option<(&str, &str)> {
    let (unit, last) = logical.strip_prefix("sources/inputs/")?.rsplit_once('/')?;
    (RECORDS.contains(&last) && valid_unit(unit)).then_some((unit, last))
}

/// The unit of one of a unit's records' logical names (`record_of`).
pub fn unit_of(logical: &str) -> Option<&str> {
    record_of(logical).map(|(u, _)| u)
}

/// Whether `rel` (a path relative to the NAS project folder) lies in a drop box: what the NAS root
/// the steps get refuses (crate::out::Out::path).
pub fn in_drop_box(rel: &str) -> bool {
    let rel = rel.trim_start_matches("./");
    DROP_ROOTS.iter().any(|d| rel == *d || rel.strip_prefix(d).is_some_and(|r| r.starts_with('/')))
}

/// Unit `unit`'s drop box. Only the gate lists and reads it (§7.1).
fn drop_box(root: &Path, unit: &str) -> PathBuf {
    root.join("inputs").join(unit)
}

/// A request–fulfil unit's to-do lists (`inputs/<unit>/todo/`, the app's: never read as input),
/// under the NAS project folder `root`: what crate::namestodo writes.
pub fn todo_dir(root: &Path, unit: &str) -> PathBuf {
    drop_box(root, unit).join("todo")
}

/// Where the checked copies of unit `unit` are kept (content-named, never edited).
pub fn store_dir(unit: &str) -> String {
    format!("sources/inputs/{unit}")
}

// ---- Findings ----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// The file can't be used: never accepted, the file is fixed or removed.
    Error,
    /// Usable, but likely a mistake: held until the owner accepts it.
    Warning,
}

/// What a check says about a candidate (§4.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// `<check>.<hash16>`: a hash of the check's name, what it's about by identity and what was
    /// found (`Finding::new`), never of a whole file's bytes. (A dot, not a colon: the id names the
    /// acceptance's file on the share.)
    pub id: String,
    pub level: Level,
    /// The files it's about (drop-box paths).
    pub files: Vec<String>,
    /// In plain words.
    pub message: String,
    /// Where it is, when that's a place (longitude, latitude).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    /// The lines it flags (line number from 1, the line), for a finding about lines of a file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<(usize, String)>,
}

impl Finding {
    /// A finding of check `check` (a short name: `gt-line`), about `about` (identities: a file's
    /// path, a region's id), having found `found` (what makes it this finding: the flagged lines'
    /// content, the counts and the versions compared, a place rounded to 100 m).
    pub fn new(check: &str, level: Level, about: &[&str], found: &[&str], files: Vec<String>, message: String) -> Finding {
        let mut parts = vec![check];
        parts.extend(about);
        parts.push("--");
        parts.extend(found);
        Finding { id: format!("{check}.{}", store::naming::hash16(parts.join("\n").as_bytes())), level, files, message, at: None, lines: Vec::new() }
    }

    /// The check it's of (its id's first part).
    pub fn check(&self) -> &str {
        self.id.split('.').next().unwrap_or("")
    }

    /// Whether it holds what it's about, given the acceptances: errors always, warnings unless
    /// accepted.
    pub fn holds(&self, accepted: &BTreeSet<String>) -> bool {
        self.level == Level::Error || !accepted.contains(&self.id)
    }
}

/// A finding id: `<check>.<16 hex>`, the check's name lower-case letters, digits and dashes.
pub fn valid_id(id: &str) -> bool {
    let Some((c, h)) = id.split_once('.') else { return false };
    !c.is_empty() && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

// ---- A unit's checks ---------------------------------------------------------------------------

/// What one file's shape check says (`Checks::file`).
#[derive(Clone, Debug, Default)]
pub struct FileCheck {
    pub findings: Vec<Finding>,
    /// Facts read from the content (an entry count, a box), kept in the index.
    pub facts: serde_json::Map<String, serde_json::Value>,
    /// The digest of what in it affects builds (§4.6, `keyed`); None: all of its bytes.
    pub keyed: Option<String>,
    /// The other files it names (a recipe's shape file): held with it when they're held.
    pub refers: Vec<String>,
}

/// A version of a unit's files to check whole: their drop-box paths, and their bytes read when a
/// check asks (`read`).
pub struct Version<'a> {
    pub paths: BTreeSet<String>,
    pub read: &'a dyn Fn(&str) -> Result<std::sync::Arc<Vec<u8>>>,
}

/// A gate unit's checks (§4.3, §6): its shape, file by file, and its own checks over a version.
pub trait Checks: Sync {
    fn unit(&self) -> &'static str;
    /// Bumped with any change to the checks: the unit is checked again.
    fn version(&self) -> u32;
    /// Whether its drop box's subfolders are part of it (translations, descriptions); otherwise a
    /// folder there (but `todo/` and `how/`) isn't its shape.
    fn recursive(&self) -> bool {
        false
    }
    /// The shape checks of one new or changed file.
    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck;
    /// The rules run on one new or changed file by a program of their own (the translations'
    /// check.py): an error when it can't be run, which fails the check job rather than take the
    /// file unchecked.
    fn lint(&self, _path: &str, _bytes: &[u8]) -> Result<Vec<Finding>> {
        Ok(Vec::new())
    }
    /// The drop box's old place at the NAS's root, for an input moved under `inputs/` (#137): a
    /// file there is an error finding saying where to move it, once the drop box exists.
    fn old_folder(&self) -> Option<&'static str> {
        None
    }
    /// What a removal says: the file as accepted (its entry in the index).
    fn removed(&self, _path: &str, _was: &FileEntry) -> Vec<Finding> {
        Vec::new()
    }
    /// The unit's own checks over a whole version (the candidate, or the next version: §4.3).
    fn whole(&self, _v: &Version) -> Result<Vec<Finding>> {
        Ok(Vec::new())
    }
    /// The files held together with `path` (a register's `.geojson` with its `.toml`), of `all`.
    fn partners(&self, _path: &str, _all: &BTreeSet<String>) -> Vec<String> {
        Vec::new()
    }
    /// Whether it takes a file of its drop box at all (the regions take their region files, shapes
    /// and `pins.toml` alone): a file it doesn't take is ignored, neither listed nor read.
    fn takes(&self, _path: &str) -> bool {
        true
    }
    /// Whether it takes a file whose name every drop box passes over (`ignored`): the regions'
    /// `_group.toml`, a folder's group's name (§6.1.2).
    fn takes_ignored(&self, _name: &str) -> bool {
        false
    }
    /// Whether a finding of `whole` about a file that didn't change holds it too (kept as accepted
    /// and shown as held): the regions' pins, which a new pass makes wrong without any file
    /// changing (docs/inputs.md §6.1.5). `whole` then runs on every check, changes or none.
    fn holds_unchanged(&self) -> bool {
        false
    }
    /// What the check needs made before it can take the files it holds for it (the regions'
    /// extract's ids, `r<id>` and `w<id>`), as the last `whole` found: in the held report.
    fn needs(&self) -> Vec<String> {
        Vec::new()
    }
    /// What's derived from the next version whole (§4.3, after step 5): facts for its files (set
    /// over the file check's, with the `keyed` digest when given), and files to store with them
    /// (content name → bytes: the regions' stored geometry). `keep`: the files whose accepted entry
    /// stays as it is (held), given nothing.
    fn derive(&self, _v: &Version, _keep: &BTreeSet<String>) -> Result<Derived> {
        Ok(Derived::default())
    }
    /// Why the check can't run now (the regions' while the current pass has no outline set yet):
    /// the job then changes nothing, and runs again when its key does. None: ready.
    fn not_ready(&self) -> Option<String> {
        None
    }
    /// The bytes to record as the unit's `@switched` (`switched_logical`), when its readers are to
    /// switch to accepted version `index` now (the regions': the migration's proof says the same of
    /// exactly that index); None otherwise. Asked only while the records have none.
    fn switch(&self, _root: &Path, _index: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    /// The content names of what else its checks read from the records (the pass's outlines): in
    /// the check's key.
    fn context(&self, _manifest: &BTreeMap<String, String>) -> Vec<String> {
        Vec::new()
    }
    /// The checks the check job runs, given what they read from the records `manifest` names (the
    /// languages' the pass's outlines' codes); None: these as they are.
    fn prepared(&self, _root: &Path, _manifest: &BTreeMap<String, String>) -> Result<Option<Box<dyn Checks>>> {
        Ok(None)
    }
}

/// What `Checks::derive` gives.
#[derive(Clone, Debug, Default)]
pub struct Derived {
    /// By drop-box path: facts set over the file check's, and the `keyed` digest (None: as it was).
    pub facts: BTreeMap<String, (serde_json::Map<String, serde_json::Value>, Option<String>)>,
    /// Files stored with the version, by their content names (under `sources/inputs/<unit>/`).
    pub store: BTreeMap<String, std::sync::Arc<Vec<u8>>>,
}

// ---- The accepted version ----------------------------------------------------------------------

/// An accepted file, in the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Its checked copy's content name (`sources/inputs/<unit>/<path>.<hash16>.<ext>`).
    pub file: String,
    pub size: u64,
    /// The digest of what in it affects builds (§4.6).
    pub keyed: String,
    /// Facts read from its content at check time.
    #[serde(default)]
    pub facts: serde_json::Map<String, serde_json::Value>,
}

impl FileEntry {
    /// The files stored with it: its copy, and those its facts name (a content name under
    /// `sources/inputs/`: a region's stored geometry), which GC keeps while it's kept.
    pub fn stored(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.file.as_str()).chain(self.facts.values().filter_map(|v| v.as_str()).filter(|s| s.starts_with("sources/inputs/") && store::naming::parse_content_name(s).is_some()))
    }
}

/// A unit's accepted version (§4.6): `sources/inputs/<unit>/index.<hash16>.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub fmt: u32,
    pub unit: String,
    /// The checks it was taken with (`<unit> <version>`).
    pub checks: String,
    /// By drop-box path.
    pub files: BTreeMap<String, FileEntry>,
    /// The warnings it was taken with, by id, each with the files it's about: kept while those
    /// files are in the version unchanged (an acceptance of none of them is stale).
    #[serde(default)]
    pub accepted: BTreeMap<String, Vec<String>>,
}

/// The check's own state (`sources/inputs/<unit>/@listed.<hash16>.json`, `listed_logical`): the
/// sizes and times of a unit's files as its last check listed them, what the next check compares a
/// listing with, reading only the files that differ (a held change's file keeps the time of the
/// version accepted, so it's read again; a file only touched takes its new time, so it isn't); and
/// the index the last change of version replaced, which GC keeps, with its files, while this state
/// is recent (gc::inputs_kept), so a version replaced stays restorable.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub fmt: u32,
    pub unit: String,
    pub listed: BTreeMap<String, (u64, u64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<String>,
    /// When the last full check (every file hashed) ran (seconds since the epoch): the lead runs
    /// one daily from it, whichever Mac led then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_at: Option<u64>,
}

/// A unit's check's state, as the records name it (empty before its first check).
pub fn read_listed(root: &Path, manifest: &BTreeMap<String, String>, unit: &str) -> Result<Listed> {
    let Some(name) = manifest.get(&listed_logical(unit)) else { return Ok(Listed { fmt: 1, unit: unit.into(), ..Default::default() }) };
    read_listed_file(root, name)
}

/// A check's state, by its content name.
pub fn read_listed_file(root: &Path, name: &str) -> Result<Listed> {
    let b = std::fs::read(root.join(name)).with_context(|| format!("read {name}"))?;
    serde_json::from_slice::<Listed>(&b).with_context(|| format!("parse {name}"))
}

/// A unit's name as one component of a file name (`timetables/gtfs` → `timetables+gtfs`: `+` is in
/// no unit's name).
pub fn flat(unit: &str) -> String {
    unit.replace('/', "+")
}

/// The report of a check that held something (`inputs-held/<unit>`): `sources/inputs/<unit>/
/// held.<hash16>.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub fmt: u32,
    pub unit: String,
    pub checks: String,
    /// The changes held (drop-box paths: a held new file is absent from the version, a held edit
    /// or removal keeps the file as accepted).
    pub held: Vec<String>,
    /// The findings that hold them.
    pub findings: Vec<Finding>,
    /// Why every change was held together, when the next version checked whole raised a finding
    /// (§4.3, step 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub together: Option<String>,
    /// Every finding id the check raised, accepted or not (for the acceptances no candidate raises
    /// any more).
    #[serde(default)]
    pub raised: Vec<String>,
    /// What the check needs made first (`Checks::needs`: the regions' extract's ids), which the
    /// lead plans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
}

/// A unit's accepted version as a step reads it: the files by their drop-box paths.
#[derive(Clone, Debug)]
pub struct Accepted {
    root: PathBuf,
    /// The index's content name: the unit's version.
    pub version: String,
    pub index: Index,
}

impl Accepted {
    /// The checked copy of drop-box path `path`.
    pub fn path(&self, path: &str) -> Option<PathBuf> {
        self.index.files.get(path).map(|f| self.root.join(&f.file))
    }

    /// Every accepted file: (drop-box path, its checked copy).
    pub fn files(&self) -> impl Iterator<Item = (&String, PathBuf)> + '_ {
        self.index.files.iter().map(|(p, f)| (p, self.root.join(&f.file)))
    }

    /// The bytes of drop-box path `path`.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let p = self.path(path).with_context(|| format!("{path} isn't in {}'s accepted version", self.index.unit))?;
        std::fs::read(&p).with_context(|| format!("read {}", p.display()))
    }
}

/// Unit `unit`'s accepted files, as the records `manifest` a step planned with name them: the one
/// way a step reads an input (docs/inputs.md §4.6, §7.1). None before any version is accepted.
pub fn open(root: &Path, manifest: &BTreeMap<String, String>, unit: &str) -> Result<Option<Accepted>> {
    let Some(version) = manifest.get(&logical(unit)) else { return Ok(None) };
    let index = read_index(root, version)?;
    anyhow::ensure!(index.unit == unit, "{version} is {}'s index, not {unit}'s", index.unit);
    Ok(Some(Accepted { root: root.to_path_buf(), version: version.clone(), index }))
}

/// An index, by its content name.
pub fn read_index(root: &Path, name: &str) -> Result<Index> {
    let b = std::fs::read(root.join(name)).with_context(|| format!("read {name}"))?;
    serde_json::from_slice(&b).with_context(|| format!("parse {name}"))
}

/// A held report, by its content name.
pub fn read_report(root: &Path, name: &str) -> Result<Report> {
    let b = std::fs::read(root.join(name)).with_context(|| format!("read {name}"))?;
    serde_json::from_slice(&b).with_context(|| format!("parse {name}"))
}

/// The content name a drop-box file's checked copy takes: its path up to its file name's first dot
/// (the logical name, which has none), the hash, and the rest of the name (the extension; `bin`
/// for a name without a dot). None when the path can't be a content name (a backslash in it).
pub fn copy_name(unit: &str, path: &str, hash16: &str) -> Option<String> {
    let (dir, name) = path.rsplit_once('/').map_or(("", path), |(d, n)| (d, n));
    let (stem, ext) = name.split_once('.').unwrap_or((name, "bin"));
    let logical = if dir.is_empty() { format!("{}/{stem}", store_dir(unit)) } else { format!("{}/{dir}/{stem}", store_dir(unit)) };
    let n = format!("{logical}.{hash16}.{ext}");
    store::naming::parse_content_name(&n).is_some_and(|c| c.logical == logical).then_some(n)
}

/// The hash in a content name.
pub fn hash_of(content: &str) -> Option<&str> {
    store::naming::parse_content_name(content).map(|c| c.hash16)
}

// ---- Listing a drop box ------------------------------------------------------------------------

/// A drop box as listed: each file's size and time (seconds), by path, and what isn't its shape's
/// (a folder in a unit without them).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub files: BTreeMap<String, (u64, u64)>,
    #[serde(default)]
    pub strays: Vec<String>,
    /// Files in the drop box's old place (`Checks::old_folder`), by their paths from the NAS's
    /// root: each an error finding saying where to move it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub misplaced: Vec<String>,
}

/// Whether a name in a drop box is passed over: the Mac's and the NAS's own files (`.DS_Store`,
/// `@eaDir`, `#recycle`) and the owner's drafts (`_draft.toml`).
pub fn ignored(name: &str) -> bool {
    name.starts_with(['.', '@', '#', '_'])
}

/// Unit `unit`'s drop box listed now (`todo/` and `how/` aside, and the names `ignored`): an empty
/// listing when there's no drop box yet, an error when it can't be listed now.
pub fn list(root: &Path, unit: &str, recursive: bool) -> Result<Listing> {
    let top = drop_box(root, unit);
    let mut l = Listing::default();
    match std::fs::metadata(&top) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(l),
        Err(e) => return Err(e).with_context(|| format!("list {}", top.display())),
        Ok(_) => {}
    }
    let mut stack = vec![(top.clone(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        for e in std::fs::read_dir(&dir).with_context(|| format!("list {}", dir.display()))? {
            let e = e.with_context(|| format!("list {}", dir.display()))?;
            let name = e.file_name().to_string_lossy().into_owned();
            if ignored(&name) && !checks(unit).is_some_and(|c| c.takes_ignored(&name)) {
                continue;
            }
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let md = e.metadata().with_context(|| format!("stat {}", e.path().display()))?;
            if !md.is_dir() && checks(unit).is_some_and(|c| !c.takes(&path)) {
                continue;
            }
            if md.is_dir() {
                if rel.is_empty() && (name == "todo" || name == "how") {
                    continue;
                }
                if recursive {
                    stack.push((e.path(), path));
                } else {
                    l.strays.push(format!("{path}/"));
                }
                continue;
            }
            let t = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            l.files.insert(path, (md.len(), t));
        }
    }
    l.strays.sort();
    // (The drop box's old place, once the drop box is there: anything in it, at any depth, was
    // meant for the drop box, and is never passed over quietly.)
    if let Some(old) = checks(unit).and_then(|c| c.old_folder()) {
        let mut stack = vec![(root.join(old), old.to_string())];
        while let Some((dir, rel)) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
            };
            for e in rd {
                let e = e.with_context(|| format!("list {}", dir.display()))?;
                let name = e.file_name().to_string_lossy().into_owned();
                if ignored(&name) {
                    continue;
                }
                let path = format!("{rel}/{name}");
                if e.file_type().with_context(|| format!("stat {}", e.path().display()))?.is_dir() {
                    // (An older app's to-do lists, `todo/` at its top: the app's, not a drop.)
                    if !(rel == old && name == "todo") {
                        stack.push((e.path(), path));
                    }
                } else {
                    l.misplaced.push(path);
                }
            }
        }
        l.misplaced.sort();
    }
    Ok(l)
}

/// What of listing `now` (made at `at`, seconds) has held still: the files not changed in the last
/// `QUIET_S`, by their time alone (a file the same in two listings a few seconds apart may still be
/// being written). One changed since is left for the next listing.
pub fn settle(now: &Listing, at: u64) -> Listing {
    let files = now.files.iter().filter(|(_, &(_, t))| t + QUIET_S <= at).map(|(p, v)| (p.clone(), *v)).collect();
    Listing { files, strays: now.strays.clone(), misplaced: now.misplaced.clone() }
}

/// What a check is given to check (`scenic-build inputs --listing`): the listing and acceptances
/// its key was made from, so the key recorded is the one of what the check used.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Planned {
    pub listing: Listing,
    pub accepted: BTreeSet<String>,
}

// ---- Acceptances -------------------------------------------------------------------------------

/// An acceptance (`state/inputs/accepted/<unit>/<id>.json`), written once (create-new) by the
/// member asking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Acceptance {
    pub id: String,
    /// The member (its id, or "-" when it has none) and its host's name.
    pub member: String,
    pub host: String,
    /// When (seconds since the epoch), and how it was asked.
    pub at: u64,
    pub by: String,
    /// The finding's message as it was.
    pub message: String,
}

fn accepted_dir(root: &Path, unit: &str) -> PathBuf {
    root.join(ACCEPTED).join(unit)
}

/// The ids accepted for unit `unit` (its acceptances' file names): none when there are none yet,
/// an error when they can't be listed now.
pub fn acceptances(root: &Path, unit: &str) -> Result<BTreeSet<String>> {
    let d = accepted_dir(root, unit);
    let rd = match std::fs::read_dir(&d) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(e).with_context(|| format!("list {}", d.display())),
    };
    let mut out = BTreeSet::new();
    for e in rd {
        let name = e?.file_name().to_string_lossy().into_owned();
        if let Some(id) = name.strip_suffix(".json").filter(|id| valid_id(id)) {
            out.insert(id.to_string());
        }
    }
    Ok(out)
}

/// Accepts warning `f` of unit `unit` (create-new: one already there is left as it was): whether
/// it was written now. Errors can't be accepted.
pub fn accept(root: &Path, unit: &str, f: &Finding, member: &str, by: &str) -> Result<bool> {
    if f.level == Level::Error {
        bail!("{} is an error: errors can't be accepted; fix or remove {}", f.id, f.files.join(", "));
    }
    anyhow::ensure!(valid_id(&f.id), "{:?} isn't a finding id", f.id);
    anyhow::ensure!(valid_unit(unit), "{unit:?} isn't a unit's name");
    let d = accepted_dir(root, unit);
    std::fs::create_dir_all(&d).with_context(|| format!("create {}", d.display()))?;
    let a = Acceptance { id: f.id.clone(), member: member.to_string(), host: crate::agent::cond::host_name(), at: crate::agent::jobs::now_s(), by: by.to_string(), message: f.message.clone() };
    let body = serde_json::to_vec_pretty(&a)?;
    let p = d.join(format!("{}.json", f.id));
    match std::fs::File::options().write(true).create_new(true).open(&p) {
        Ok(mut file) => {
            std::io::Write::write_all(&mut file, &body).with_context(|| format!("write {}", p.display()))?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e).with_context(|| format!("create {}", p.display())),
    }
}

/// Removes the acceptance of `id` for unit `unit`: whether there was one.
pub fn unaccept(root: &Path, unit: &str, id: &str) -> Result<bool> {
    anyhow::ensure!(valid_id(id), "{id:?} isn't a finding id");
    anyhow::ensure!(valid_unit(unit), "{unit:?} isn't a unit's name");
    match std::fs::remove_file(accepted_dir(root, unit).join(format!("{id}.json"))) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).context("remove the acceptance"),
    }
}

/// An acceptance's file, read.
pub fn read_acceptance(root: &Path, unit: &str, id: &str) -> Result<Acceptance> {
    let p = accepted_dir(root, unit).join(format!("{id}.json"));
    serde_json::from_slice(&std::fs::read(&p).with_context(|| format!("read {}", p.display()))?).with_context(|| format!("parse {}", p.display()))
}

/// This Mac's member id as its agent keeps it (crate::pool::member_id), never made here: "-" when
/// it has none yet.
pub fn member_of(agent_home: &Path) -> String {
    std::fs::read_to_string(agent_home.join("member")).ok().and_then(|s| s.lines().next().map(str::trim).filter(|id| crate::pool::is_member_id(id)).map(str::to_string)).unwrap_or_else(|| "-".into())
}

// ---- The check's key ---------------------------------------------------------------------------

/// The key of unit `unit`'s check (§4.3): the step's and the checks' versions, the listing (a
/// trigger: names, sizes, times), the acceptances, the accepted index named now and whatever else
/// its checks read. When it changes, the unit is checked again.
pub fn check_key(checks: &dyn Checks, listing: &Listing, accepted: &BTreeSet<String>, manifest: &BTreeMap<String, String>) -> String {
    let mut parts = vec![format!("inputs {INPUTS_V}"), format!("{} {}", checks.unit(), checks.version())];
    parts.extend(listing.files.iter().map(|(p, (s, t))| format!("{p} {s} {t}")));
    parts.extend(listing.strays.iter().map(|p| format!("stray {p}")));
    parts.extend(listing.misplaced.iter().map(|p| format!("misplaced {p}")));
    parts.push(format!("accepted {}", accepted.iter().cloned().collect::<Vec<_>>().join(",")));
    parts.push(format!("index {}", manifest.get(&logical(checks.unit())).map(String::as_str).unwrap_or("-")));
    parts.extend(checks.context(manifest));
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    crate::agent::build::h(&refs)
}

/// The asks of the gate the agent takes up (`inputs-request.json` in its folder): from the menu bar,
/// the map, the build page (through the coordinator) and `scenic inputs check`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    pub unit: String,
    /// Finding ids to accept; `all`: every warning held now.
    #[serde(default)]
    pub accept: Vec<String>,
    #[serde(default)]
    pub all: bool,
    #[serde(default)]
    pub unaccept: Vec<String>,
    /// List the drop boxes now (`full`: hash every file of the unit).
    #[serde(default)]
    pub check: bool,
    #[serde(default)]
    pub full: bool,
    /// Remove the unit's records (a unit off the gate: `scenic inputs test off --forget`).
    #[serde(default)]
    pub forget: bool,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub at: u64,
}

/// The asks' folder in the agent's folder: a file each, taken up in name order.
pub const ASKS: &str = "inputs-asks";

/// Leaves an ask for the agent whose folder is `home`.
pub fn ask(home: &Path, a: &Ask) -> Result<()> {
    let d = home.join(ASKS);
    std::fs::create_dir_all(&d)?;
    let name = format!("{:020}-{}-{}.json", a.at, std::process::id(), store::naming::hash16(&serde_json::to_vec(a)?));
    crate::whole::write(&d.join(name), &serde_json::to_vec(a)?)
}

/// The asks waiting in `home`, taken (each removed once read; one that doesn't parse goes).
pub fn take_asks(home: &Path) -> Vec<Ask> {
    let d = home.join(ASKS);
    let Ok(rd) = std::fs::read_dir(&d) else { return Vec::new() };
    let mut names: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    names.sort();
    let mut out = Vec::new();
    for p in names {
        if let Some(a) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Ask>(&b).ok()) {
            out.push(a);
        }
        std::fs::remove_file(&p).ok();
    }
    out
}

/// Carries out an ask's acceptances (`held`: the unit's held report, for the findings' messages and
/// `all`): what was done, in words.
pub fn apply_ask(root: &Path, a: &Ask, held: Option<&Report>, member: &str) -> Result<Vec<String>> {
    let mut said = Vec::new();
    let findings: Vec<&Finding> = held.map(|r| r.findings.iter().collect()).unwrap_or_default();
    let want: Vec<&Finding> = if a.all { findings.iter().copied().filter(|f| f.level == Level::Warning).collect() } else { a.accept.iter().map(|id| findings.iter().copied().find(|f| &f.id == id).with_context(|| format!("no finding {id} of {} is held now", a.unit))).collect::<Result<_>>()? };
    for f in want {
        if accept(root, &a.unit, f, member, &a.by)? {
            said.push(format!("accepted {} ({})", f.id, f.message));
        }
    }
    for id in &a.unaccept {
        if unaccept(root, &a.unit, id)? {
            said.push(format!("unaccepted {id}"));
        }
    }
    Ok(said)
}

#[cfg(test)]
mod tests;
