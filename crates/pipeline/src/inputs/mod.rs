//! The drop boxes and the checking gate (docs/inputs.md §3, §4). Every input the map is built from
//! has a folder on the NAS, its drop box (`inputs/<unit>/`), where the owner drops it in its shape.
//! A change is checked before it's taken in: the `inputs` step (crate::inputs::gate), one target per
//! gate unit, a job like any other (it hands off; the lead merges). A change with an unaccepted
//! finding is held, and the last accepted version stays in use. The build reads only the accepted
//! version: checked copies, content-named under `sources/inputs/<unit>/`, listed by an index whose
//! content name is the unit's version, named in the records as `sources/inputs/<unit>/index` (`open`, the one
//! accessor: §7.1).
//!
//! The parts: the units and their checks (`Checks`; the test unit's, crate::inputs::gatetest); the
//! drop boxes' listing (`list`, `settle`), which the agent makes every two minutes off its loop
//! (crate::inputs::watch); the check job (crate::inputs::gate); the acceptances (`accept`,
//! `unaccept`); the status's entries (crate::inputs::view); the credits from the inputs'
//! descriptions (crate::inputs::credits).
//!
//! On the gate (docs/inputs.md §4.10): the test unit `_gate-test` alone, while
//! `state/inputs/gate-test` exists (`TEST_FLAG`). Each real input joins `UNITS` in its own task
//! (#135–#145).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub mod credits;
pub mod gate;
pub mod gatetest;
pub mod view;
pub mod watch;

/// The `inputs` step's version: bumping it checks every unit again.
pub const INPUTS_V: u32 = 1;

/// The gate units on the gate (each input's task adds its own).
pub const UNITS: [&str; 0] = [];

/// The test unit (docs/inputs.md §4.10): not real data; on the gate while `TEST_FLAG` exists.
pub const TEST_UNIT: &str = "_gate-test";

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

/// The units on the gate now: `UNITS`, and the test unit while its flag is there.
pub fn units(root: &Path) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = UNITS.to_vec();
    if root.join(TEST_FLAG).exists() {
        v.push(TEST_UNIT);
    }
    v
}

/// The checks of unit `unit`; None for a unit this app doesn't know.
pub fn checks(unit: &str) -> Option<&'static dyn Checks> {
    match unit {
        TEST_UNIT => Some(&gatetest::GateTest),
        _ => None,
    }
}

/// The records' entry naming unit `unit`'s accepted index (`sources/inputs/<unit>/index`: a
/// content name is its logical name's, as the lead checks every entry's at merge).
pub fn logical(unit: &str) -> String {
    format!("{}/index", store_dir(unit))
}

/// The records' entry naming unit `unit`'s report while a change is held.
pub fn held_logical(unit: &str) -> String {
    format!("{}/held", store_dir(unit))
}

/// The unit of a records entry `sources/inputs/<unit>/index` or `…/held`.
pub fn unit_of(logical: &str) -> Option<&str> {
    let rest = logical.strip_prefix("sources/inputs/")?;
    rest.strip_suffix("/index").or_else(|| rest.strip_suffix("/held")).filter(|u| !u.is_empty() && !u.contains('/'))
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
    /// The content names of what else its checks read from the records (the pass's outlines): in
    /// the check's key.
    fn context(&self, _manifest: &BTreeMap<String, String>) -> Vec<String> {
        Vec::new()
    }
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

/// A unit's accepted version (§4.6): `sources/inputs/<unit>/index.<hash16>.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub fmt: u32,
    pub unit: String,
    /// The checks it was taken with (`<unit> <version>`).
    pub checks: String,
    /// By drop-box path.
    pub files: BTreeMap<String, FileEntry>,
    /// Each file's size and time as listed when it was taken (what a later check compares a
    /// listing with, reading only what differs).
    #[serde(default)]
    pub listed: BTreeMap<String, (u64, u64)>,
    /// The ids of the warnings it was taken with.
    #[serde(default)]
    pub accepted: Vec<String>,
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
            if ignored(&name) {
                continue;
            }
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let md = e.metadata().with_context(|| format!("stat {}", e.path().display()))?;
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
    Ok(l)
}

/// What of listing `now` (made at `at`, seconds) has held still: files as the listing before
/// (`before`) had them, or not changed in the last `QUIET_S`. One changed since is left for the next
/// listing (as is a stray seen once).
pub fn settle(before: Option<&Listing>, now: &Listing, at: u64) -> Listing {
    let files = now.files.iter().filter(|(p, &(size, t))| before.is_some_and(|b| b.files.get(*p) == Some(&(size, t))) || t + QUIET_S <= at).map(|(p, v)| (p.clone(), *v)).collect();
    Listing { files, strays: now.strays.clone() }
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
