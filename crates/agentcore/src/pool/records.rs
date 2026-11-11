//! The build's records (docs/pool.md §6.2): the manifest, the job keys, the uploads not yet
//! verified, the raw tiles' archives waiting to be named, and the hand-offs they reflect, numbered
//! by the epoch of the lead's store they were saved in (docs/coordination.md §4.3, step 8c) and a
//! sequence number one more each save. The lead keeps them on its own disk first (step 8b: what its
//! take-up starts from), then writes them whole to one path on the NAS, `state/build/records.json`,
//! and its brief, which every reader reads (buildstore::records::now). A reader never pairs keys
//! from one version with a manifest of another (invariant 4). A save landing late over the one
//! path (one an earlier process left running as it stopped) is read until the lead's next save
//! (`driver::RESAVE_S` at most), and never taken up: the lead's own copy is newer
//! (crate::pool::driver's take-up).
//!
//! A take-up (crate::pool::driver) starts from the newest of its lead's own records and the one
//! path's, by (epoch, seq); in a new pool, from today's files (`Records::today`). Each target keeps
//! the lease that last set it, so an older lease's hand-off merged later (a member back from
//! sleep, a replay) doesn't undo a newer one's.
//!
//! Which file a reader reads them from (`ONE`, `ONE_BRIEF`, today's files before the pool) is
//! buildstore::records's, generic over what's read (task #916 step 6a, docs/crates.md §6): here,
//! the whole records.

use super::journal::{Entry, LeaseId};
use super::nas::Nas;
use anyhow::Result;
use buildstore::rawpack::Pack;
use buildstore::records::{read_or_default, Snapshot, TODAY};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub use buildstore::records::{ONE, ONE_BRIEF};

/// The keys of the jobs that last succeeded, by step and target ("3/4/2", "6/31/20").
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Keys {
    /// Terrain-hi's and slope-hi's z6 tiles, per z6 tile ("6/x/y": `terrain_slope_targets`). Their steps
    /// were named "terrain" and "slope" before task #639: records under those names are theirs
    /// (`RENAMED`).
    #[serde(default, alias = "terrain")]
    pub terrain_hi: BTreeMap<String, String>,
    #[serde(default, alias = "slope")]
    pub slope_hi: BTreeMap<String, String>,
    /// Their deep levels, per z10 tile ("10/x/y": `terrain_slope_targets`).
    #[serde(default)]
    pub terrain_deep: BTreeMap<String, String>,
    #[serde(default)]
    pub slope_deep: BTreeMap<String, String>,
    /// Their assemblies, per z3 tile ("3/x/y").
    #[serde(default)]
    pub terrain_lo: BTreeMap<String, String>,
    #[serde(default)]
    pub slope_lo: BTreeMap<String, String>,
    /// The `unit` step's z6 tiles ("6/x/y"). (Its name is stored, in `jobs.json` and everywhere a step
    /// is named: kept, docs/pipeline.md §2.)
    #[serde(default)]
    pub unit: BTreeMap<String, String>,
    /// The z6 tiles' landmark candidates.
    #[serde(default)]
    pub pois: BTreeMap<String, String>,
    /// The z6 tiles' peaks' prominence and isolation.
    #[serde(default)]
    pub peaks: BTreeMap<String, String>,
    #[serde(default)]
    pub pack: BTreeMap<String, String>,
    #[serde(default)]
    pub lo: BTreeMap<String, String>,
    /// The tree cover layers' z6 tiles, per z6 tile ("6/x/y", `trees_targets`).
    #[serde(default)]
    pub trees: BTreeMap<String, String>,
    /// Their assemblies, per z3 tile ("3/x/y").
    #[serde(default)]
    pub trees_lo: BTreeMap<String, String>,
    /// The 3D buildings' normalized files (`bldprep`) and tiles (`bldtiles`), per z6 tile ("6/x/y":
    /// `bld_targets`).
    #[serde(default)]
    pub bldprep: BTreeMap<String, String>,
    #[serde(default)]
    pub bldtiles: BTreeMap<String, String>,
    /// The buildings field's lo levels, per z3 tile ("3/x/y": `bld_targets`).
    #[serde(default)]
    pub bldfield_lo: BTreeMap<String, String>,
    /// The depth's z6 tiles, per z6 tile ("6/x/y"), and its assemblies, per z3 tile ("3/x/y":
    /// `depth_targets`).
    #[serde(default)]
    pub depth: BTreeMap<String, String>,
    #[serde(default)]
    pub depth_lo: BTreeMap<String, String>,
    /// The lakes' estimated depth, per z6 tile ("6/x/y": `depth_targets`' `lakes`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lake_depth: BTreeMap<String, String>,
    /// The elevation pyramids' tiles stored on the NAS (terrain::pyramids), per z6 tile ("6/x/y":
    /// `terrain_slope_targets`' `pyramids`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pyramids_fill: BTreeMap<String, String>,
    /// The polar caps' blocks ("s/4/x/y": terrain::polar::target) and roots ("s"): `polar_targets`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub polar_terrain: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub polar_slope: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub polar_root: BTreeMap<String, String>,
    /// The served files the last catalogue was made from.
    #[serde(default, rename = "catalog")] // the stored key keeps its old spelling (task #547)
    pub catalogue: Option<String>,
    /// The same for the last catalogue held for review (`inputs/hold-catalog`: written to
    /// the NAS's map-held/, not served).
    #[serde(default, rename = "catalog_held")] // stored key: old spelling
    pub catalogue_held: Option<String>,
    /// Records of steps this app doesn't know (a newer app's), kept as they are when the keys are
    /// saved again: an older app's agent leading the build doesn't drop them.
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
    /// For planning (`load_with`; never saved): the files the hand-offs waiting to be merged save,
    /// by logical name. A tree cover z6 tile's mid there counts as made (`tree_work`).
    #[serde(skip)]
    pub handed: BTreeSet<String>,
}

impl Keys {
    /// The keys for showing (the status): none when they can't be read now.
    pub fn load(root: &store::nasfs::NasRoot) -> Keys {
        Self::load_strict(root).unwrap_or_default()
    }

    /// The keys for planning and recording: none when there are none yet, an error when they can't
    /// be read now (so a job isn't started again, nor the keys written back, from empty ones).
    pub fn load_strict(root: &store::nasfs::NasRoot) -> anyhow::Result<Keys> {
        Ok(crate::pool::records::Brief::now(root)?.keys)
    }

    /// The keys with the done records of the hand-offs waiting under each of `bases` on top (the
    /// coordinator's journal on this Mac: buildstore::handoff), and the files they save (`handed`).
    pub fn load_with(root: &store::nasfs::NasRoot, bases: &[std::path::PathBuf]) -> anyhow::Result<Keys> {
        Keys::load_strict(root)?.with_waiting(bases)
    }

    /// `load_with`, on keys read already (with the manifest, from one read of the records).
    pub fn with_waiting(mut self, bases: &[std::path::PathBuf]) -> anyhow::Result<Keys> {
        let mut hs = Vec::new();
        for b in bases {
            hs.extend(buildstore::handoff::waiting_in(b)?);
        }
        let k = &mut self;
        for (_, h) in hs {
            for (l, c) in &h.changes {
                if c.is_some() {
                    k.handed.insert(l.clone());
                } else {
                    k.handed.remove(l);
                }
            }
            if let Some((step, targets)) = &h.done {
                k.record(step, targets);
            }
        }
        Ok(self)
    }

    pub fn save(&self, root: &store::nasfs::NasRoot) -> anyhow::Result<()> {
        let p = root.join("state/build/jobs.json");
        p.parent().unwrap().create_dir_all()?;
        p.write_whole(serde_json::to_vec_pretty(self)?)
    }

    /// The keys of per-target step `step` (by its current name), by target; None for a step that
    /// keeps none per target.
    pub fn per_target(&mut self, step: &str) -> Option<&mut BTreeMap<String, String>> {
        Some(match current(step) {
            "terrain-hi" => &mut self.terrain_hi,
            "slope-hi" => &mut self.slope_hi,
            "terrain-deep" => &mut self.terrain_deep,
            "slope-deep" => &mut self.slope_deep,
            "terrain-lo" => &mut self.terrain_lo,
            "slope-lo" => &mut self.slope_lo,
            "unit" => &mut self.unit,
            "pois" => &mut self.pois,
            "peaks" => &mut self.peaks,
            "pack" => &mut self.pack,
            "trees" => &mut self.trees,
            "trees-lo" => &mut self.trees_lo,
            "bldprep" => &mut self.bldprep,
            "bldtiles" => &mut self.bldtiles,
            "bldfield-lo" => &mut self.bldfield_lo,
            "depth" => &mut self.depth,
            "depth-lo" => &mut self.depth_lo,
            "lake-depth" => &mut self.lake_depth,
            "pyramids-fill" => &mut self.pyramids_fill,
            "polar-terrain" => &mut self.polar_terrain,
            "polar-slope" => &mut self.polar_slope,
            "polar-root" => &mut self.polar_root,
            _ => return None,
        })
    }

    /// The key recorded for `target` of a per-target step.
    pub fn recorded(&self, step: &str, target: &str) -> Option<&str> {
        let m = match current(step) {
            "terrain-hi" => &self.terrain_hi,
            "slope-hi" => &self.slope_hi,
            "terrain-deep" => &self.terrain_deep,
            "slope-deep" => &self.slope_deep,
            "terrain-lo" => &self.terrain_lo,
            "slope-lo" => &self.slope_lo,
            "unit" => &self.unit,
            "pois" => &self.pois,
            "peaks" => &self.peaks,
            "pack" => &self.pack,
            "lo" => &self.lo,
            "trees" => &self.trees,
            "trees-lo" => &self.trees_lo,
            "bldprep" => &self.bldprep,
            "bldtiles" => &self.bldtiles,
            "bldfield-lo" => &self.bldfield_lo,
            "depth" => &self.depth,
            "depth-lo" => &self.depth_lo,
            "lake-depth" => &self.lake_depth,
            "pyramids-fill" => &self.pyramids_fill,
            "polar-terrain" => &self.polar_terrain,
            "polar-slope" => &self.polar_slope,
            "polar-root" => &self.polar_root,
            _ => return None,
        };
        m.get(target).map(String::as_str)
    }

    /// Records a job's targets as done with their keys. A prune forgets its targets' keys instead
    /// ("unit 6/x/y", "pois 6/x/y", "pack 6/x/y", "lo 3/x/y", "bldprep 6/x/y", "bldtiles 6/x/y",
    /// "bldfield-lo 3/x/y"), so a region added back is built again; a redo (`scenic redo`,
    /// crate::agent::redo) forgets any per-target step's ("terrain-hi 6/x/y"), so it's built again.
    pub fn record(&mut self, step: &str, done: &[(String, String)]) {
        if step == "prune" {
            for (t, _) in done {
                let Some((kind, at)) = t.split_once(' ') else { continue };
                match kind {
                    "unit" => {
                        self.unit.remove(at);
                    }
                    "pois" => {
                        self.pois.remove(at);
                        self.peaks.remove(at);
                    }
                    "pack" => {
                        self.pack.remove(at);
                    }
                    "lo" => {
                        self.lo.remove(at);
                    }
                    "bldprep" => {
                        self.bldprep.remove(at);
                    }
                    "bldtiles" => {
                        self.bldtiles.remove(at);
                    }
                    "bldfield-lo" => {
                        self.bldfield_lo.remove(at);
                    }
                    _ => {}
                }
            }
            return;
        }
        if step == store::catalogue::NAME {
            self.catalogue = done.first().map(|d| d.1.clone());
            return;
        }
        if step == store::catalogue::HELD_NAME {
            self.catalogue_held = done.first().map(|d| d.1.clone());
            return;
        }
        if step == crate::agent::redo::STEP {
            // ("<step> <target>": a redo, its key forgotten. A third word is a restore's or 4a's
            // scheme-2 save, "<step> <target> <from>", done before step 4b (docs/readiness.md §4):
            // a hand-off of one still waiting records nothing, never forgets its target.)
            for (t, _) in done {
                let mut w = t.split(' ');
                let (Some(s), Some(at), None) = (w.next(), w.next(), w.next()) else { continue };
                if let Some(m) = self.per_target(s) {
                    m.remove(at);
                }
            }
            return;
        }
        // (A step keeping no key per target, the worldwide steps', the roots' and the lo's: kept with
        // the lo keys, under the step's own name.)
        let m = if self.per_target(step).is_some() { self.per_target(step).unwrap() } else { &mut self.lo };
        for (t, k) in done {
            m.insert(t.clone(), k.clone());
        }
    }
}

/// The steps renamed (task #639: by the pack each makes), old name to new. What an older app
/// recorded under an old name (the records' keys, a hand-off's done targets, an older app's hand-off) is
/// the step's under its new one (`current`), so nothing is built again for the name alone.
pub const RENAMED: [(&str, &str); 2] = [("terrain", "terrain-hi"), ("slope", "slope-hi")];

/// `step`'s name now: its new name where it was renamed (`RENAMED`), else itself.
pub fn current(step: &str) -> &str {
    RENAMED.iter().find(|(old, _)| *old == step).map_or(step, |(_, new)| new)
}

/// A check of a hand-off before it's applied, given the records it would apply to (§7.3: its
/// writes within its step's write-set, its content names matching their logical names, its lease's
/// targets and keys): why it's refused, if it is; or, its why beginning `HOLD`, why it's held
/// (neither applied nor refused: merged again later, `Merged::held`).
pub type Check<'a> = &'a dyn Fn(&Entry, &Records) -> std::result::Result<(), String>;

/// What a check's why begins with when it holds an entry rather than refuse it (`Check`): an
/// entry from a member on a newer app than the lead's, which may write what this app's
/// write-sets don't know of (crate::agent::pool::check_from).
pub const HOLD: &str = "held: ";

/// The build's records.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Records {
    /// The epoch of the lead's store they were saved in (docs/coordination.md §4.3): under the key
    /// `term`, as when they were numbered by the pool's terms (step 8c: the first epoch is past
    /// every term made, the records' term the floor of a take-up's epoch).
    #[serde(rename = "term")]
    pub epoch: u64,
    /// The snapshot's number in its epoch: one more each save, from 1 (0: none saved yet).
    pub seq: u64,
    /// Logical name → content name.
    pub manifest: BTreeMap<String, String>,
    /// The keys of the jobs that last succeeded.
    pub keys: Keys,
    /// Uploads not yet verified on the NAS: content name → SHA-256.
    pub pending: BTreeMap<String, String>,
    /// The sizes of the files the manifest names, as their jobs wrote them (content name → bytes;
    /// buildstore::out::SIZES): a catalogue's, without a look at the NAS. Records from before them have
    /// none, nor files written before.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sizes: BTreeMap<String, u64>,
    /// Raw terrain tiles' and stored pyramids' archives handed off, to be named in their stores'
    /// indexes (buildstore::rawpack::handed_place): the lead names them
    /// (buildstore::rawpack::name_handed_places), then takes them off here (crate::pool::driver::Heard::
    /// named_raw), saved with its next snapshot. One named but not yet off (a restart between) is
    /// named again: passed over unread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<(String, Pack)>,
    /// The hand-offs applied (or passed over: `apply`), by key (crate::pool::journal::Entry::key).
    #[serde(default)]
    pub reflected: BTreeSet<String>,
    /// The hand-offs refused, with why.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rejected: BTreeMap<String, String>,
    /// The lease of the last done record applied, by step and target (`slots`): an entry of an older
    /// lease for one of them, merged later, changes nothing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub last: BTreeMap<String, BTreeMap<String, LeaseId>>,
}

/// A snapshot's manifest and keys alone (a quarter of its bytes: the unverified uploads, the sizes
/// and the hand-offs' names left out), written whole by the lead after each snapshot (`ONE_BRIEF`),
/// for the readers that need no more (`Brief::now`): the map server's syncs, the hosts, the agent's
/// planning, a catalog's checks. One read pairs keys and manifest of one version (invariant 4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Brief {
    /// The epoch it's of (`Records::epoch`).
    #[serde(rename = "term")]
    pub epoch: u64,
    /// The snapshot it's of (`Records::seq`).
    pub seq: u64,
    /// Logical name → content name.
    pub manifest: BTreeMap<String, String>,
    /// The keys of the jobs that last succeeded.
    pub keys: Keys,
}

impl Brief {
    /// `Records::now`'s manifest and keys: the one path's brief; today's two files before the pool
    /// (buildstore::records::brief_now).
    pub fn now(nas: &dyn Nas) -> Result<Brief> {
        buildstore::records::brief_now::<Brief>(nas)
    }
}

impl buildstore::records::Brief for Brief {
    fn today(nas: &dyn Nas) -> Result<Brief> {
        Ok(Brief {
            manifest: read_or_default(nas, TODAY[0])?,
            keys: read_or_default(nas, TODAY[1])?,
            ..Default::default()
        })
    }
}

impl Snapshot for Records {
    fn today(nas: &dyn Nas) -> Result<Records> {
        Records::today(nas)
    }
}

/// The slots a done record's target `t` of step `step` is kept under, for lease order (`last`): a
/// prune's or a redo's `<kind> <at>` removes kind's key for `at` (and a pois prune the peaks' too:
/// `Keys::record`; a redo's kind under its old names too, as a member on an
/// older app names its builds: `RENAMED`); any other target is its step's.
fn slots(step: &str, t: &str) -> Vec<(String, String)> {
    let redo = step == crate::agent::redo::STEP;
    match t.split_once(' ').filter(|_| step == "prune" || redo) {
        Some(("pois", at)) if !redo => vec![("pois".into(), at.into()), ("peaks".into(), at.into())],
        Some((kind, at)) if redo => std::iter::once(kind)
            .chain(RENAMED.iter().filter(|(_, new)| *new == kind).map(|(old, _)| *old))
            .map(|k| (k.to_string(), at.to_string()))
            .collect(),
        Some((kind, at)) => vec![(kind.into(), at.into())],
        None => vec![(step.into(), t.into())],
    }
}

/// The one path's snapshot as read now (buildstore::records::load_one); None when there's none: a
/// lead's take-up (crate::pool::driver).
pub fn load_one(nas: &dyn Nas) -> Result<Option<Records>> {
    buildstore::records::load_one(nas)
}

/// The (epoch, seq) of the one path's brief (`ONE_BRIEF`) as read now; None when there's none: a
/// lead's take-up's fence (crate::pool::driver). A save writes its brief only once its snapshot is
/// written whole at the one path, so a snapshot older than the brief there is one that landed late
/// over a newer save's (an earlier lead's write left running): its brief's epoch and seq say so.
pub fn brief_mark(nas: &dyn Nas) -> Result<Option<(u64, u64)>> {
    #[derive(Deserialize)]
    struct Mark {
        term: u64,
        seq: u64,
    }
    let Some(b) = nas.read(ONE_BRIEF)? else { return Ok(None) };
    let m: Mark = serde_json::from_slice(&b).map_err(|e| anyhow::anyhow!("parse {ONE_BRIEF}: {e}"))?;
    Ok(Some((m.term, m.seq)))
}

impl Records {
    /// The build's records as today's three files hold them (before the pool: its first take-up's).
    /// The pending read before the manifest (a save writes the manifest first: every upload the
    /// pending read holds is one the manifest read after has named, unless replaced since; GC's drop
    /// of the uploads no name reads counts on it).
    pub fn today(nas: &dyn Nas) -> Result<Records> {
        let pending = read_or_default(nas, TODAY[2])?;
        Ok(Records {
            manifest: read_or_default(nas, TODAY[0])?,
            keys: read_or_default(nas, TODAY[1])?,
            pending,
            sizes: read_or_default(nas, buildstore::out::SIZES)?,
            ..Default::default()
        })
    }

    /// The build's records as a reader has them now (§6.2; buildstore::records::now): the one
    /// path's; before the pool, today's files.
    pub fn now(nas: &dyn Nas) -> Result<Records> {
        buildstore::records::now(nas)
    }

    /// Saves them as the next snapshot (`seq` one more): kept by `keep` first (the lead's own
    /// disk, what its take-up starts from: docs/coordination.md step 8b), nothing written to the
    /// NAS unless that's done; then whole to the one path and its brief (`ONE`, `ONE_BRIEF`), the
    /// brief only while the declaration (read between) still names `lead` (when given): a save
    /// that lands late, its lead moved meanwhile, leaves the next lead's brief, so a take-up's
    /// fence (`brief_mark`) sees the one path's records older than it (task #1038, the
    /// simulator: a lead asleep mid-save wrote both over the next lead's, and a take-up from them
    /// lost what that lead acknowledged). A read that fails writes it, as before. The number goes
    /// up even when a write fails, so no two versions that may land share one.
    pub fn save(&mut self, nas: &dyn Nas, keep: Keep, lead: Option<&str>) -> std::result::Result<(), Unsaved> {
        if self.epoch < 1 {
            return Err(Unsaved::Kept(anyhow::anyhow!("records of no epoch")));
        }
        self.seq += 1;
        // (The sizes of files the manifest no longer names let go.)
        let named: BTreeSet<&String> = self.manifest.values().collect();
        self.sizes.retain(|c, _| named.contains(c));
        let b = serde_json::to_vec(self).map_err(|e| Unsaved::Kept(e.into()))?;
        keep(&b).map_err(Unsaved::Kept)?;
        self.put(nas, &b, lead).map_err(Unsaved::Nas)
    }

    /// Their NAS files, `b` the snapshot's bytes (`save`).
    fn put(&self, nas: &dyn Nas, b: &[u8], lead: Option<&str>) -> Result<()> {
        // (Brief's fields, borrowed.)
        #[derive(Serialize)]
        struct B<'a> {
            term: u64,
            seq: u64,
            manifest: &'a BTreeMap<String, String>,
            keys: &'a Keys,
        }
        let brief =
            serde_json::to_vec(&B { term: self.epoch, seq: self.seq, manifest: &self.manifest, keys: &self.keys })?;
        nas.write_whole(ONE, b)?;
        if let (Some(me), Ok(Some(d))) = (lead, buildstore::records::lead::declared(nas)) {
            anyhow::ensure!(
                d.member == me,
                "{ONE} written, its brief not: {} is the declared lead now (this save landed late)",
                d.host
            );
        }
        nas.write_whole(ONE_BRIEF, &brief)
    }

    /// Whether hand-off `key` is in them: applied (or passed over), or refused.
    pub fn handles(&self, key: &str) -> bool {
        self.reflected.contains(key) || self.rejected.contains_key(key)
    }

    /// Applies hand-off `e`, under key `key`, as buildstore::handoff's merge did: its manifest
    /// changes, its uploads pending and checked, its done record in the keys, its raw archives to
    /// name; and names it. Passed over (named, nothing applied: false) when a later lease's entry
    /// set any of its targets already (lease order across merges: a member back from sleep posts
    /// an older lease's hand-off after a newer one's was merged).
    pub fn apply(&mut self, key: &str, e: &Entry) -> bool {
        let h = &e.handoff;
        let slots: Vec<(String, String)> =
            h.done.iter().flat_map(|(step, ts)| ts.iter().flat_map(|(t, _)| slots(step, t))).collect();
        if slots.iter().any(|(s, t)| self.last.get(s).and_then(|m| m.get(t)).is_some_and(|l| *l > e.lease)) {
            self.reflected.insert(key.to_string());
            return false;
        }
        for (k, v) in &h.changes {
            match v {
                Some(n) => self.manifest.insert(k.clone(), n.clone()),
                None => self.manifest.remove(k),
            };
            // (Its sizes of the files it names alone.)
            if let Some((n, size)) = v.as_ref().and_then(|n| h.sizes.get(n).map(|z| (n, *z))) {
                self.sizes.insert(n.clone(), size);
            }
        }
        self.pending.extend(h.pending.iter().map(|(k, v)| (k.clone(), v.clone())));
        for c in &h.checked {
            self.pending.remove(c);
        }
        if let Some((step, targets)) = &h.done {
            self.keys.record(step, targets);
        }
        for r in &h.raw {
            if !self.raw.contains(r) {
                self.raw.push(r.clone());
            }
        }
        for (s, t) in slots {
            self.last.entry(s).or_default().insert(t, e.lease);
        }
        self.reflected.insert(key.to_string());
        true
    }

    /// Names hand-off `key` refused, with why.
    pub fn refuse(&mut self, key: &str, why: &str) {
        self.rejected.insert(key.to_string(), why.to_string());
    }
}

/// What a merge did, by hand-off key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    /// Applied to the records.
    pub applied: Vec<String>,
    /// Named in the records with nothing applied: a later lease's hand-off set their targets already.
    pub overtaken: Vec<String>,
    /// Refused, with why (named so in the records).
    pub refused: Vec<(String, String)>,
    /// Held by the check (`HOLD`), with why: neither applied nor refused; to merge later.
    pub held: Vec<(String, String)>,
    /// Of those applied, each one's job done (its key, member, step and targets), for publishing's
    /// times (crate::pubtimes).
    pub done: Vec<MergedJob>,
}

/// A merged hand-off's job done: its key, member, step and targets.
pub type MergedJob = (String, String, String, Vec<String>);

/// Merges hand-offs `entries` the records don't name yet, in lease order (one without a key, its
/// time not a day, passed over: none is kept so): each checked and applied, or refused, or held
/// (`HOLD`). Nothing is written: the caller saves the records, then acknowledges what the saved
/// records name.
pub fn merge<'a>(r: &mut Records, entries: impl IntoIterator<Item = &'a Entry>, check: Check) -> Merged {
    let mut out = Merged::default();
    let todo: BTreeMap<(LeaseId, String), &Entry> = entries
        .into_iter()
        .filter_map(|e| Some(((e.lease, e.key()?), e)))
        .filter(|((_, k), _)| !r.handles(k))
        .collect();
    for ((_, k), e) in todo {
        match check(e, r) {
            Ok(()) => {
                if r.apply(&k, e) {
                    if let Some((step, targets)) = &e.handoff.done {
                        out.done.push((
                            k.clone(),
                            e.member.clone(),
                            step.clone(),
                            targets.iter().map(|t| t.0.clone()).collect(),
                        ));
                    }
                    out.applied.push(k);
                } else {
                    out.overtaken.push(k);
                }
            }
            Err(why) => match why.strip_prefix(HOLD) {
                Some(held) => out.held.push((k, held.to_string())),
                None => {
                    r.refuse(&k, &why);
                    out.refused.push((k, why));
                }
            },
        }
    }
    out
}

/// A take-up's error when the records it reads are older than those its lead saved last
/// (crate::pool::driver: a stale read, or its disk's copy restored from a backup).
#[derive(Debug)]
pub struct Older(pub String);

impl std::fmt::Display for Older {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Older {}

/// Where a save keeps the records on its lead's own disk before the NAS (`Records::save`): the
/// driver's `Io::keep`; nothing in a test of the NAS's files alone.
pub type Keep<'a> = &'a dyn Fn(&[u8]) -> Result<()>;

/// A save that didn't end (`Records::save`): not kept on the lead's disk (nothing written to the
/// NAS), or kept and not written whole to the NAS (its take-up done all the same; its NAS files
/// tried again, nothing acknowledged meanwhile).
#[derive(Debug)]
pub enum Unsaved {
    Kept(anyhow::Error),
    Nas(anyhow::Error),
}

impl std::fmt::Display for Unsaved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unsaved::Kept(e) => write!(f, "not kept on this Mac's disk: {e:#}"),
            Unsaved::Nas(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for Unsaved {}

#[cfg(test)]
mod tests {
    use super::super::journal::LeaseId;
    use super::super::nas::Mem;
    use super::*;
    use buildstore::handoff::Handoff;

    const DAY: u64 = 1_791_300_000;

    /// Saves `r` to the NAS alone.
    fn save(r: &mut Records, nas: &dyn Nas) -> std::result::Result<(), Unsaved> {
        r.save(nas, &|_| Ok(()), None)
    }

    /// An entry of lease `epoch`-`n` building unit `t` with key `k`.
    fn built(epoch: u64, n: u64, t: &str, k: &str) -> Entry {
        let content = format!("base/{t}.{k}.base");
        let h = Handoff {
            changes: [(format!("base/{t}"), Some(content.clone()))].into(),
            pending: [(content, "sha".into())].into(),
            done: Some(("unit".into(), vec![(t.into(), k.into())])),
            ..Default::default()
        };
        Entry {
            member: "m-000000000000000b".into(),
            lease: LeaseId { epoch, n },
            step: "unit".into(),
            handoff: h,
            at: DAY + n,
        }
    }

    fn any(_: &Entry, _: &Records) -> std::result::Result<(), String> {
        Ok(())
    }

    /// The sizes an entry hands off are kept for the files it names alone, and let go once the
    /// manifest no longer names them; an entry without sizes still merges.
    #[test]
    fn the_records_keep_the_sizes_of_the_files_the_manifest_names() {
        let nas = Mem::default();
        let mut r = Records { epoch: 1, ..Default::default() };
        let mut e = built(1, 1, "6-1-1", "k1");
        e.handoff.sizes = [("base/6-1-1.k1.base".into(), 12), ("base/6-9-9.k0.base".into(), 3)].into();
        assert!(r.apply("a", &e));
        assert!(r.apply("b", &built(1, 2, "6-1-2", "k2")));
        assert_eq!(r.sizes, [("base/6-1-1.k1.base".to_string(), 12)].into());
        let mut again = built(1, 3, "6-1-1", "k3");
        again.handoff.sizes = [("base/6-1-1.k3.base".into(), 14)].into();
        assert!(r.apply("c", &again));
        save(&mut r, &nas).unwrap();
        assert_eq!(r.sizes, [("base/6-1-1.k3.base".to_string(), 14)].into());
        assert_eq!(Records::now(&nas).unwrap().sizes, r.sizes);
    }

    /// docs/coordination.md step 8c: a save writes the one path and its brief alone (no term's own
    /// files, nor today's), under the key `term` older apps read; readers read them back, today's
    /// files only before the pool, never once a lead is declared and the one path can't be read.
    #[test]
    fn a_save_writes_the_one_path_and_its_brief_alone() {
        let nas = Mem::default();
        nas.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        assert_eq!((Brief::now(&nas).unwrap().manifest.len(), Records::now(&nas).unwrap().manifest.len()), (1, 1));
        let mut r = Records { epoch: 841, ..Records::today(&nas).unwrap() };
        let e = built(841, 1, "6-1-2", "k1");
        r.apply(&e.key().unwrap(), &e);
        save(&mut r, &nas).unwrap();
        let files: Vec<String> =
            nas.0.borrow().keys().filter_map(|k| k.strip_prefix("state/build/")).map(str::to_string).collect();
        assert_eq!(files, ["brief.json", "manifest.json", "records.json"], "today's manifest as it was");
        let brief = Brief::now(&nas).unwrap();
        assert_eq!((brief.epoch, brief.seq, &brief.manifest, &brief.keys), (841, 1, &r.manifest, &r.keys));
        assert_eq!(Records::now(&nas).unwrap(), r);
        let v: serde_json::Value = serde_json::from_slice(&nas.read(ONE).unwrap().unwrap()).unwrap();
        assert_eq!((v["term"].as_u64(), v["seq"].as_u64()), (Some(841), Some(1)), "{v}");
        // The one path not read (a stale read) once a lead is declared: an error, never today's.
        let fresh = Mem::default();
        fresh.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        fresh.write_whole(buildstore::records::lead::LEAD, br#"{"member": "m-000000000000000a"}"#).unwrap();
        assert!(Records::now(&fresh).is_err() && Brief::now(&fresh).is_err());
        assert!(Records { epoch: 0, ..r }.save(&nas, &|_| Ok(()), None).is_err(), "records of no epoch");
    }

    /// Task #1038: a save whose lead the declaration no longer names (it landed late, the lead
    /// moved meanwhile) writes the one path and not its brief: the next lead's brief stays, newer,
    /// for a take-up's fence (`brief_mark`).
    #[test]
    fn a_save_landing_after_a_move_leaves_the_next_leads_brief() {
        let nas = Mem::default();
        let lead = |m: &str| {
            let d = format!("{{\"member\": \"{m}\", \"host\": \"{m}\"}}");
            nas.write_whole(buildstore::records::lead::LEAD, d.as_bytes()).unwrap()
        };
        lead("a");
        let mut next = Records { epoch: 1002, ..Default::default() };
        next.save(&nas, &|_| Ok(()), Some("a")).unwrap();
        assert_eq!(brief_mark(&nas).unwrap(), Some((1002, 1)));
        lead("b");
        let mut late = Records { epoch: 1, seq: 7, ..Default::default() };
        let err = late.save(&nas, &|_| Ok(()), Some("a")).unwrap_err();
        assert!(matches!(err, Unsaved::Nas(_)), "{err}");
        assert_eq!(
            serde_json::from_slice::<Records>(&nas.read(ONE).unwrap().unwrap()).unwrap().epoch,
            1,
            "the one path's written"
        );
        assert_eq!(brief_mark(&nas).unwrap(), Some((1002, 1)), "its brief not");
    }

    #[test]
    fn a_merge_applies_in_lease_order_and_refuses_what_its_check_refuses() {
        let mut r = Records { epoch: 3, ..Default::default() };
        // Handed in another order than leased: the later lease's key wins.
        let (a, b) = (built(3, 9, "6-1-1", "k9"), built(3, 2, "6-1-1", "k2"));
        let bad = Entry { step: "bogus".into(), ..built(3, 5, "6-1-3", "kx") };
        let check = |e: &Entry, _: &Records| {
            if e.step == "bogus" {
                Err("writes outside its step's names".to_string())
            } else {
                Ok(())
            }
        };
        let m = merge(&mut r, [&a, &b, &bad, &a], &check);
        assert_eq!(m.applied, [b.key().unwrap(), a.key().unwrap()]);
        assert_eq!(m.refused, [(bad.key().unwrap(), "writes outside its step's names".to_string())]);
        assert_eq!(r.keys.unit.get("6-1-1").map(String::as_str), Some("k9"));
        assert_eq!(r.manifest.get("base/6-1-1").map(String::as_str), Some("base/6-1-1.k9.base"));
        assert!(!r.manifest.contains_key("base/6-1-3") && r.handles(&bad.key().unwrap()) && r.pending.len() == 2);
        assert_eq!(r.rejected[&bad.key().unwrap()], "writes outside its step's names", "why, in the records");
        // Handed again: nothing more.
        assert_eq!(merge(&mut r, [&a, &b, &bad], &check), Merged::default());
        // Held by its check: neither applied nor refused, merged again later.
        let c = built(3, 11, "6-1-4", "k11");
        let hold = |_: &Entry, _: &Records| Err::<(), String>(format!("{HOLD}a newer app's"));
        let m = merge(&mut r, [&c], &hold);
        assert_eq!(m.held, [(c.key().unwrap(), "a newer app's".to_string())]);
        assert!(!r.handles(&c.key().unwrap()));
        assert_eq!(merge(&mut r, [&c], &any).applied, [c.key().unwrap()]);
    }

    /// Merges hand-off `e` alone: its key, and what the merge did.
    fn merge1(r: &mut Records, e: &Entry, check: Check) -> (String, Merged) {
        (e.key().unwrap(), merge(r, [e], check))
    }

    #[test]
    fn an_older_lease_merged_after_a_newer_one_is_passed_over() {
        // (Review M5: lease order held only within one merge, and an older lease's entry merged
        // later won.)
        let mut r = Records { epoch: 3, ..Default::default() };
        let (newer, m) = merge1(&mut r, &built(3, 9, "6-1-1", "k9"), &any);
        assert_eq!(m.applied, [newer]);
        let (older, m) = merge1(&mut r, &built(3, 5, "6-1-1", "k5"), &any);
        assert_eq!((m.applied.len(), m.overtaken.clone()), (0, vec![older.clone()]));
        assert_eq!(r.keys.unit.get("6-1-1").map(String::as_str), Some("k9"), "the newer lease's key");
        assert_eq!(r.manifest.get("base/6-1-1").map(String::as_str), Some("base/6-1-1.k9.base"));
        assert!(r.handles(&older), "named: it doesn't wait");
        // An older lease's entry for other targets, and a later prune of the unit, apply; a build
        // older than the prune doesn't bring it back.
        let (other, m) = merge1(&mut r, &built(3, 4, "6-1-2", "k4"), &any);
        assert_eq!(m.applied, [other]);
        let h = Handoff {
            changes: [("base/6-1-1".to_string(), None)].into(),
            done: Some(("prune".into(), vec![("unit 6-1-1".into(), String::new())])),
            ..Default::default()
        };
        let (prune, m) =
            merge1(&mut r, &Entry { step: "prune".into(), handoff: h, ..built(3, 12, "6-1-1", "-") }, &any);
        assert_eq!(m.applied, [prune]);
        let (late, m) = merge1(&mut r, &built(3, 10, "6-1-1", "k10"), &any);
        assert_eq!(m.overtaken, [late]);
        assert!(!r.manifest.contains_key("base/6-1-1") && !r.keys.unit.contains_key("6-1-1"));
        assert_eq!(r.last["unit"]["6-1-1"], LeaseId { epoch: 3, n: 12 });
        // Kept in the snapshot, for the next lead.
        assert_eq!(serde_json::from_slice::<Records>(&serde_json::to_vec(&r).unwrap()).unwrap(), r);
    }

    /// Task #780: a redo's entry (`scenic redo`: a job of the lead's, no file changed) passes the
    /// lead's check and forgets the key of the target it names alone, the manifest as it was; a
    /// build leased before it, merged after, is passed over, and one leased after applies; the
    /// same redo again changes nothing.
    #[test]
    fn a_redo_forgets_its_targets_keys_alone() {
        let mut r = Records { epoch: 3, ..Default::default() };
        let check = |e: &Entry, r: &Records| crate::agent::pool::check(crate::agent::seam::for_tests(), e, r);
        assert_eq!(merge(&mut r, &[built(3, 1, "6-1-1", "k1"), built(3, 2, "6-1-2", "k2")], &any).applied.len(), 2);
        let redo = |n: u64| {
            let h = Handoff {
                done: Some((
                    crate::agent::redo::STEP.into(),
                    vec![(crate::agent::redo::done_target("unit", "6-1-1"), String::new())],
                )),
                ..Default::default()
            };
            Entry { step: crate::agent::redo::STEP.into(), handoff: h, ..built(3, n, "-", "-") }
        };
        let (manifest, before) = (r.manifest.clone(), r.keys.clone());
        let (ts, marks) = (["6-1-1".to_string()], crate::agent::redo::marks(&r, "unit", &["6-1-1".to_string()]));
        let (k, m) = merge1(&mut r, &redo(5), &check);
        // (#789: what `scenic redo --run` follows: the redo's lease is the target's mark now.)
        assert_ne!(crate::agent::redo::marks(&r, "unit", &ts), marks);
        assert_eq!(r.last["unit"]["6-1-1"], LeaseId { epoch: 3, n: 5 });
        assert!(m.refused.is_empty() && m.applied == [k], "{:?}", m.refused);
        assert_eq!(r.keys.unit.get("6-1-1"), None);
        assert_eq!(r.keys.unit.get("6-1-2").map(String::as_str), Some("k2"), "the other target's kept");
        assert_eq!(r.manifest, manifest, "no file changed");
        let (late, m) = merge1(&mut r, &built(3, 4, "6-1-1", "k4"), &any);
        assert_eq!(m.overtaken, [late]);
        assert_eq!(r.keys.unit.get("6-1-1"), None, "a build leased before the redo doesn't bring its key back");
        let again = r.keys.clone();
        merge1(&mut r, &redo(6), &check);
        assert_eq!(r.keys, again, "a second redo changes nothing");
        // (Review: a redo's slots are its step's alone, under its old names too.)
        assert_eq!(slots("redo", "pois 6/1/1"), [("pois".to_string(), "6/1/1".to_string())]);
        assert_eq!(
            slots("redo", "terrain-hi 6/1/1"),
            [("terrain-hi".to_string(), "6/1/1".to_string()), ("terrain".into(), "6/1/1".into())]
        );
        let (next, m) = merge1(&mut r, &built(3, 7, "6-1-1", "k1"), &any);
        assert_eq!(m.applied, [next]);
        assert_eq!(r.keys, before, "built again under its key");
    }
}
