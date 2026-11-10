//! The build's records (docs/pool.md §6.2): the manifest, the job keys, the uploads not yet
//! verified, the raw tiles' archives waiting to be named, and the journal entries they reflect,
//! numbered by their term and a sequence number one more each save. The lead keeps them on its
//! own disk first (docs/coordination.md step 8b: what its take-up starts from), then writes them
//! whole to one path on the NAS, `state/build/records.json` (and its brief), and to its term's own
//! snapshot, `state/build/term/<E>/records.json` (its brief there too while a member heard reads
//! those: the rollout gate). A reader never pairs keys from one version with a manifest of another
//! (invariant 4). A save of an earlier term's lead landing late over the one path is passed over:
//! a term after its own is made, so a reader or a take-up from the NAS reads the later term's own
//! (invariant 5); one of this term's (an earlier process's take-up of it) is read until the lead's
//! next save (`driver::RESAVE_S` at most).
//!
//! Term 1 is today's layout, `state/build/manifest.json`, `jobs.json` and `pending.json` (nothing
//! moves when the pool begins): its first snapshot is made from them before term 1 itself is, in a
//! file of its own that no lead writes (`first`: `term/1/first.json`), and they're written after
//! each of term 1's snapshots, for today's readers. Term 1's take-up, and a reader, read its lead's
//! snapshot, then its first, and today's files again only once the first has stayed unreadable
//! longer than any stale read (its maker stopped midway, or its bytes landed out of order), no
//! lead's snapshot there: three files can be read half rewritten, one stale and another not. Its
//! maker finishing it late lands where no one reads once its lead has saved.
//!
//! Taking up a term (`start`, then the merge and the save) starts from the records of the term
//! before: its lead's own, kept on its disk, when it led that term; else (another Mac led it, or
//! its own copy is lost) the newest snapshot a read finds. It replays the journal
//! entries they don't name in (term, lease) order, and saves that as the new term's first snapshot.
//! A stale read only sends an entry to the replay, or to its member's telling the new lead again
//! (crate::pool::journal::Mine): none is lost (invariant 3). Each target keeps the lease that last
//! set it, so an older lease's entry merged later (a member back from sleep, a replay) doesn't undo
//! a newer one's.
//!
//! Which file a reader reads them from (the paths, `load`, `newest`, `now`, `Brief::now`, term 1's
//! fallbacks) is buildstore::records's, generic over what's read (task #916 step 6a, docs/crates.md
//! §6): here, the whole records.

use super::journal::{self, Entry, LeaseId};
use super::nas::{Created, Nas};
use super::term::Term;
use anyhow::{bail, Context, Result};
use buildstore::rawpack::Pack;
use buildstore::records::{read_or_default, BriefOf, Snapshot, TODAY};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub use buildstore::records::{brief_path, path, FirstUnreadable, FIRST, ONE, ONE_BRIEF};

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
/// recorded under an old name (the records' keys, a hand-off's done targets, a journal entry) is
/// the step's under its new one (`current`), so nothing is built again for the name alone.
pub const RENAMED: [(&str, &str); 2] = [("terrain", "terrain-hi"), ("slope", "slope-hi")];

/// `step`'s name now: its new name where it was renamed (`RENAMED`), else itself.
pub fn current(step: &str) -> &str {
    RENAMED.iter().find(|(old, _)| *old == step).map_or(step, |(_, new)| new)
}

/// A check of a journal entry before it's applied, given the records it would apply to (§7.3: its
/// writes within its step's write-set, its content names matching their logical names, its lease's
/// targets and keys): why it's refused, if it is; or, its why beginning `HOLD`, why it's held
/// (neither applied nor refused: merged again later, `Merged::held`).
pub type Check<'a> = &'a dyn Fn(&Entry, &Records) -> std::result::Result<(), String>;

/// What a check's why begins with when it holds an entry rather than refuse it (`Check`): an
/// entry from a member on a newer app than the lead's, which may write what this app's
/// write-sets don't know of (crate::agent::pool::check_from).
pub const HOLD: &str = "held: ";

/// A term's records.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Records {
    /// The term they're of.
    pub term: u64,
    /// The snapshot's number in its term: one more each save, from 1 (0: none saved yet).
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
    /// The journal entries applied (or passed over: `apply`), by key (crate::pool::journal::Entry::key).
    #[serde(default)]
    pub reflected: BTreeSet<String>,
    /// The journal entries refused, with why.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rejected: BTreeMap<String, String>,
    /// The lease of the last done record applied, by step and target (`slots`): an entry of an older
    /// lease for one of them, merged later, changes nothing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub last: BTreeMap<String, BTreeMap<String, LeaseId>>,
}

/// A snapshot's manifest and keys alone (a quarter of its bytes: the unverified uploads, the sizes
/// and the journal's names left out), written whole by the lead after each snapshot (`ONE_BRIEF`;
/// a term's own past the first while the gate writes them), for the readers that need no more (`Brief::now`): the map server's syncs,
/// the hosts, the agent's planning, a catalog's checks. One read pairs keys and manifest of one
/// version (invariant 4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Brief {
    /// The term it's of.
    pub term: u64,
    /// The snapshot it's of (`Records::seq`).
    pub seq: u64,
    /// Logical name → content name.
    pub manifest: BTreeMap<String, String>,
    /// The keys of the jobs that last succeeded.
    pub keys: Keys,
}

impl Brief {
    /// `Records::now`'s manifest and keys: the one path's brief while it's the current term's, else a
    /// term's brief where it has one (its snapshot where it has none); today's two files before the
    /// pool (buildstore::records::brief_now).
    pub fn now(nas: &dyn Nas) -> Result<Brief> {
        buildstore::records::brief_now::<Records, Brief>(nas)
    }
}

impl BriefOf<Records> for Brief {
    fn term(&self) -> u64 {
        self.term
    }

    fn of(r: Records) -> Brief {
        Brief { term: r.term, seq: r.seq, manifest: r.manifest, keys: r.keys }
    }

    fn today(nas: &dyn Nas) -> Result<Brief> {
        Ok(Brief {
            term: 1,
            manifest: read_or_default(nas, TODAY[0])?,
            keys: read_or_default(nas, TODAY[1])?,
            ..Default::default()
        })
    }
}

impl Snapshot for Records {
    fn term(&self) -> u64 {
        self.term
    }

    fn today(nas: &dyn Nas) -> Result<Records> {
        Records::today(nas)
    }

    fn with_seq(self, seq: u64) -> Records {
        Records { seq, ..self }
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

impl Records {
    /// Term `term`'s snapshot, as read now (buildstore::records::load); None when it has none.
    pub fn load(nas: &dyn Nas, term: u64) -> Result<Option<Records>> {
        buildstore::records::load(nas, term)
    }

    /// The terms' own snapshots as a reader walks them while term `term` is current, the one path
    /// left out (buildstore::records::newest_over): the tests' look at them.
    #[cfg(test)]
    pub fn newest(nas: &dyn Nas, term: u64, today: bool) -> Result<Option<Records>> {
        buildstore::records::newest_over(nas, term, None, today)
    }

    /// The build's records as today's three files hold them (term 1's, as the pool begins). The
    /// pending read before the manifest (a save writes the manifest first: every upload the pending
    /// read holds is one the manifest read after has named, unless replaced since; GC's drop of the
    /// uploads no name reads counts on it).
    pub fn today(nas: &dyn Nas) -> Result<Records> {
        let pending = read_or_default(nas, TODAY[2])?;
        Ok(Records {
            term: 1,
            manifest: read_or_default(nas, TODAY[0])?,
            keys: read_or_default(nas, TODAY[1])?,
            pending,
            sizes: read_or_default(nas, buildstore::out::SIZES)?,
            ..Default::default()
        })
    }

    /// The build's records as a reader has them now (§6.2; buildstore::records::now): the one path's
    /// while it's the current term's, else the current term's own snapshot, or the newest before
    /// it; before the pool's first term, today's files.
    pub fn now(nas: &dyn Nas) -> Result<Records> {
        buildstore::records::now(nas)
    }

    /// Saves them as the next snapshot (`seq` one more): kept by `keep` first (the lead's own
    /// disk, what its take-up starts from: docs/coordination.md step 8b), nothing written to the
    /// NAS unless that's done; then whole to the one path and its brief (`ONE`, `ONE_BRIEF`), and to
    /// their term's own snapshot (`path`: only this term's lead writes it, so a late write of an
    /// earlier term's lead, landed over the one path, never hides it from a take-up or a reader);
    /// with `per_term` (a member heard reads the terms' own briefs: the rollout gate) past term 1
    /// to the term's own brief (`brief_path`); term 1's to today's files after. The number goes up
    /// even when a write fails, so no two versions that may land share one.
    pub fn save(&mut self, nas: &dyn Nas, keep: Keep, per_term: bool) -> std::result::Result<(), Unsaved> {
        if self.term < 1 {
            return Err(Unsaved::Kept(anyhow::anyhow!("records of no term")));
        }
        self.seq += 1;
        // (The sizes of files the manifest no longer names let go.)
        let named: BTreeSet<&String> = self.manifest.values().collect();
        self.sizes.retain(|c, _| named.contains(c));
        let b = serde_json::to_vec(self).map_err(|e| Unsaved::Kept(e.into()))?;
        keep(&b).map_err(Unsaved::Kept)?;
        self.put(nas, &b, per_term).map_err(Unsaved::Nas)
    }

    /// Their NAS files, `b` the snapshot's bytes (`save`).
    fn put(&self, nas: &dyn Nas, b: &[u8], per_term: bool) -> Result<()> {
        // (Brief's fields, borrowed.)
        #[derive(Serialize)]
        struct B<'a> {
            term: u64,
            seq: u64,
            manifest: &'a BTreeMap<String, String>,
            keys: &'a Keys,
        }
        let brief =
            serde_json::to_vec(&B { term: self.term, seq: self.seq, manifest: &self.manifest, keys: &self.keys })?;
        nas.write_whole(ONE, b)?;
        nas.write_whole(ONE_BRIEF, &brief)?;
        // (The term's own snapshot, always: what fences a late write at the one path from a take-up
        // that reads the NAS, docs/coordination.md step 8b.)
        nas.write_whole(&path(self.term), b)?;
        if per_term && self.term >= 2 {
            nas.write_whole(&brief_path(self.term), &brief)?;
        }
        if self.term == 1 {
            nas.write_whole(TODAY[0], &serde_json::to_vec_pretty(&self.manifest)?)?;
            nas.write_whole(TODAY[2], &serde_json::to_vec_pretty(&self.pending)?)?;
            nas.write_whole(buildstore::out::SIZES, &serde_json::to_vec(&self.sizes)?)?;
            nas.write_whole(TODAY[1], &serde_json::to_vec_pretty(&self.keys)?)?;
        }
        Ok(())
    }

    /// Whether journal entry `key` is in them: applied (or passed over), or refused.
    pub fn handles(&self, key: &str) -> bool {
        self.reflected.contains(key) || self.rejected.contains_key(key)
    }

    /// Applies journal entry `e`, under key `key`, as buildstore::handoff's merge did: its manifest
    /// changes, its uploads pending and checked, its done record in the keys, its raw archives to
    /// name; and names it. Passed over (named, nothing applied: false) when a later lease's entry
    /// set any of its targets already (lease order across merges: a member back from sleep tells
    /// of an older lease's entry after a newer one's was merged).
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

    /// Names journal entry `key` refused, with why.
    pub fn refuse(&mut self, key: &str, why: &str) {
        self.rejected.insert(key.to_string(), why.to_string());
    }
}

/// Makes term 1's first snapshot from today's three files, unless it has one: before term 1 is made
/// (crate::pool::term::bootstrap), so every Mac that learns of term 1 reads it whole. In a file of
/// its own (`FIRST`) that no lead writes: term 1's lead saves its snapshots to its records file
/// (`path(1)`), read first. Made with create-new: of two Macs making it, the first's stays. This
/// call's create whose bytes didn't land is written whole, as `term::finish` does a term's, before
/// term 1 is made: landing late (its Mac asleep between its temporary file and its rename), it
/// lands where no one reads once term 1's lead has saved. One there already that doesn't read
/// whole (an earlier try's, its answer lost or its maker stopped; or another Mac's, its bytes not
/// landed yet, or a stale read) is left as it is: `term_one` reads today's files once it has
/// stayed so longer than any stale read.
pub fn first(nas: &dyn Nas) -> Result<()> {
    if nas.exists(FIRST)? {
        return Ok(());
    }
    let b = serde_json::to_vec(&Records { seq: 1, ..Records::today(nas)? })?;
    match nas.create_new(FIRST, &b)? {
        Created::Made | Created::There => Ok(()),
        Created::Unwritten(_) => nas.write_whole(FIRST, &b).context("finish term 1's first snapshot"),
    }
}

/// What a merge did, by journal key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    /// Applied to the records.
    pub applied: Vec<String>,
    /// Named in the records with nothing applied: a later lease's entry set their targets already.
    pub overtaken: Vec<String>,
    /// Refused, with why: their refusals to note once the records naming them are saved.
    pub refused: Vec<(String, String)>,
    /// Read and not whole (being written, not seen yet, cut short, or removed): to merge later.
    pub waiting: Vec<String>,
    /// Not read: the reading stopped (`merge_while`); to merge later.
    pub unread: Vec<String>,
    /// Not read: the read failed (the share didn't answer), which says nothing of the entry; to
    /// merge later.
    pub failed: Vec<String>,
    /// Read, and held by the check (`HOLD`), with why: neither applied nor refused; to merge later.
    pub held: Vec<(String, String)>,
    /// Of those applied, each one's job done (its key, member, step and targets), for publishing's
    /// times (crate::pubtimes).
    pub done: Vec<MergedJob>,
}

/// A merged entry's job done: its key, member, step and targets.
pub type MergedJob = (String, String, String, Vec<String>);

/// Merges the journal entries `keys` the records don't name yet: each read, checked and applied,
/// or refused, in (term, lease) order: this lead's check decides, whatever another lead's refusal
/// (crate::pool::journal::refusal). Nothing is written: the caller saves the records, then notes
/// the refusals (crate::pool::journal::note_refusal) and acknowledges what the saved records name.
pub fn merge(nas: &dyn Nas, r: &mut Records, keys: &[String], check: Check) -> Merged {
    merge_while(nas, r, keys, check, &|| true)
}

/// `merge`, reading entries only while `more()` says so (asked before each read): on a share under
/// load a read takes seconds, and a backlog of thousands must not hold one loop for an hour. The
/// keys not read are left in `Merged::unread`, to merge later; give the oldest leases first, as
/// `by_lease` orders them, so the order of what's applied holds across merges too.
pub fn merge_while(nas: &dyn Nas, r: &mut Records, keys: &[String], check: Check, more: &dyn Fn() -> bool) -> Merged {
    merge_from(&|k| journal::read(nas, k), r, keys, check, more)
}

/// `merge_while`, each entry as `read` has it: the journal's (`journal::read`), or a hand-off the
/// lead holds whole (posted by HTTP, docs/coordination.md step 6; its own jobs').
pub fn merge_from(
    read_one: &dyn Fn(&str) -> Result<journal::Read>,
    r: &mut Records,
    keys: &[String],
    check: Check,
    more: &dyn Fn() -> bool,
) -> Merged {
    let mut out = Merged::default();
    let mut read: BTreeMap<(LeaseId, String), Entry> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut stopped = false;
    for k in keys {
        if r.handles(k) || !seen.insert(k.as_str()) {
            continue;
        }
        stopped = stopped || !more();
        if stopped {
            out.unread.push(k.clone());
            continue;
        }
        match read_one(k) {
            Ok(journal::Read::Entry(e)) => {
                read.insert((e.lease, k.clone()), e);
            }
            Ok(journal::Read::Damaged(why)) => {
                r.refuse(k, &why);
                out.refused.push((k.clone(), why));
            }
            Ok(journal::Read::Short | journal::Read::Missing) => out.waiting.push(k.clone()),
            Err(_) => out.failed.push(k.clone()),
        }
    }
    for ((_, k), e) in read {
        match check(&e, r) {
            Ok(()) => {
                if r.apply(&k, &e) {
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

/// Journal keys in the order of their leases (`<day>/<term>-<n>`: by term, then number, then key):
/// the order a merge applies them in.
pub fn by_lease(keys: &mut [String]) {
    keys.sort_by_cached_key(|k| (k.rsplit('/').next().and_then(|l| l.parse::<LeaseId>().ok()), k.clone()));
}

/// The records term `t` starts from (§6.2). `own`, its lead's own (kept on its disk, or in memory: a
/// lead re-asserting; an earlier try of this take-up, its save failed, numbered on), when they're
/// of the term before `t` or of `t`: no other lead between, and nothing read (docs/coordination.md
/// step 8b: a save of its own landing late on the NAS can't send it back). Else (the owner moved
/// the lead here, or its own are lost or older) the newest a read finds: the one path's
/// (`ONE`), unless a term after it has a snapshot of its own (walking down from `t`'s, an earlier
/// try's that landed; term 1's as `term_one` says, `today` as it says: a stale read of today's files
/// pairs a manifest of one version with keys of another), or `own` where newer. Numbered as `t`'s,
/// on from a snapshot of `t`'s own, from 0 otherwise. Refused (`Older`) when what it finds is older
/// than `saved` (term, seq): its lead's last save to the NAS.
pub fn start(
    nas: &dyn Nas,
    t: &Term,
    own: Option<&Records>,
    today: bool,
    saved: Option<(u64, u64)>,
) -> Result<Records> {
    let mut base = own.filter(|o| o.term >= t.from && o.term <= t.term).cloned();
    if base.is_none() {
        let one = buildstore::records::load_one::<Records>(nas)?.filter(|o| o.term <= t.term);
        let floor = one.as_ref().map_or(1, |o| o.term + 1);
        for f in (floor..=t.term).rev() {
            if let Some(o) = own.filter(|o| o.term == f) {
                base = Some(o.clone());
                break;
            }
            let loaded = if f == 1 { Some(buildstore::records::term_one(nas, today)?) } else { Records::load(nas, f)? };
            if let Some(r) = loaded {
                base = Some(r);
                break;
            }
        }
        base = base.or(one);
        if let Some(o) = own.filter(|o| base.as_ref().is_none_or(|b| (o.term, o.seq) > (b.term, b.seq))) {
            base = Some(o.clone());
        }
    }
    let Some(mut r) = base else { bail!("no records to take term {} up from", t.term) };
    // (Never from records older than those its lead last saved: a stale read's, or one of a term
    // before. What their saves acknowledged is on no member's disk any more: docs/coordination.md
    // step 6.)
    if let Some((term, seq)) = saved.filter(|s| (r.term, r.seq) < *s) {
        return Err(Older(format!(
            "the newest records read are term {}'s at {}, older than term {term}'s at {seq}, which its lead \
             saved (a stale read: tried again)",
            r.term, r.seq
        ))
        .into());
    }
    if r.term != t.term {
        r.term = t.term;
        r.seq = 0;
    }
    Ok(r)
}

/// `start`'s error when the records it reads are older than those its lead saved last.
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
    use super::super::term::{self, Current};
    use super::super::Member;
    use super::*;
    use buildstore::handoff::Handoff;
    use std::cell::Cell;

    const DAY: u64 = 1_791_300_000;

    /// What taking up a term gave (`take_up`).
    #[derive(Clone, Debug)]
    struct TakenUp {
        records: Records,
        merged: Merged,
    }

    /// Takes up term `t` in one go, as the driver does: `start` (from `own`, as it says; `today` as
    /// it says), the journal entries `keys` replayed where the records don't name them, `check`ed
    /// first; saved as `t`'s first snapshot, and the refusals noted. When the save fails, `own`
    /// keeps the records as tried, numbered: passed again, the next try numbers on.
    fn take_up(
        nas: &dyn Nas,
        t: &Term,
        own: &mut Option<Records>,
        keys: &[String],
        check: Check,
        today: bool,
    ) -> Result<TakenUp> {
        let mut r = start(nas, t, own.as_ref(), today, None)?;
        let merged = merge(nas, &mut r, keys, check);
        if let Err(e) = r.save(nas, &|_| Ok(()), true) {
            *own = Some(r);
            return Err(e).with_context(|| format!("save term {}'s records", t.term));
        }
        *own = None;
        for (k, why) in &merged.refused {
            journal::note_refusal(nas, k, why).ok();
        }
        Ok(TakenUp { records: r, merged })
    }

    /// Saves `r` to the NAS alone, its term's own too.
    fn save(r: &mut Records, nas: &dyn Nas) -> std::result::Result<(), Unsaved> {
        r.save(nas, &|_| Ok(()), true)
    }

    /// An entry of lease `term`-`n` building unit `t` with key `k`.
    fn built(term: u64, n: u64, t: &str, k: &str) -> Entry {
        let content = format!("base/{t}.{k}.base");
        let h = Handoff {
            changes: [(format!("base/{t}"), Some(content.clone()))].into(),
            pending: [(content, "sha".into())].into(),
            done: Some(("unit".into(), vec![(t.into(), k.into())])),
            ..Default::default()
        };
        Entry {
            member: "m-000000000000000b".into(),
            lease: LeaseId { term, n },
            step: "unit".into(),
            handoff: h,
            at: DAY + n,
        }
    }

    fn any(_: &Entry, _: &Records) -> std::result::Result<(), String> {
        Ok(())
    }

    /// The sizes an entry hands off are kept for the files it names alone, let go once the manifest
    /// no longer names them, and written beside today's manifest; an entry without sizes still merges.
    #[test]
    fn the_records_keep_the_sizes_of_the_files_the_manifest_names() {
        let nas = Mem::default();
        let mut r = Records { term: 1, ..Default::default() };
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
        let today: BTreeMap<String, u64> =
            serde_json::from_slice(&nas.read(buildstore::out::SIZES).unwrap().unwrap()).unwrap();
        assert_eq!(today, r.sizes);
        assert_eq!(Records::today(&nas).unwrap().sizes, r.sizes);
    }

    fn lead(app: &str) -> Member {
        Member { id: "m-000000000000000a".into(), host: "Mac-mini".into(), app: app.into() }
    }

    /// Takes up `t` in one go, the journal listed whole (term 1's first snapshot that can't be read
    /// whole taken from today's files: it has stayed so longer than any stale read).
    fn up(nas: &dyn Nas, t: &Term, own: Option<Records>, check: Check) -> Result<TakenUp> {
        take_up(nas, t, &mut own.clone(), &journal::list(nas, None)?, check, true)
    }

    #[test]
    fn term_ones_first_snapshot_is_todays_files_and_they_follow_its_saves() {
        let nas = Mem::default();
        nas.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        nas.write_whole(TODAY[1], br#"{"unit": {"6-1-1": "k0"}}"#).unwrap();
        assert!(Records::newest(&nas, 1, false).unwrap_err().is::<FirstUnreadable>(), "none made yet");
        first(&nas).unwrap();
        let mut r = Records::newest(&nas, 1, false).unwrap().unwrap();
        assert_eq!((r.term, r.seq, r.manifest.len(), r.keys.unit.len(), r.pending.len()), (1, 1, 1, 1, 0));
        assert_eq!(r, Records { seq: 1, ..Records::today(&nas).unwrap() });
        assert_eq!(Records::load(&nas, 1).unwrap(), None, "in a file of its own, no lead's snapshot yet");
        let e = built(1, 1, "6-1-2", "k1");
        r.apply(&e.key().unwrap(), &e);
        save(&mut r, &nas).unwrap();
        assert_eq!(Records::load(&nas, 1).unwrap(), Some(r.clone()), "its lead's snapshot");
        assert_eq!(Records::newest(&nas, 1, false).unwrap(), Some(r.clone()), "read first");
        // Made once: a second Mac's try leaves it.
        let made = nas.read(FIRST).unwrap();
        first(&nas).unwrap();
        assert_eq!(nas.read(FIRST).unwrap(), made);
        // Its maker stopped between its create and its bytes: readers can't read it, and term 1's
        // take-up starts from today's files once it has stayed so longer than any stale read.
        let stopped = Mem::default();
        stopped.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        stopped.create_new(FIRST, b"").unwrap();
        assert!(Records::newest(&stopped, 1, false).unwrap_err().is::<FirstUnreadable>());
        assert_eq!(
            Records::newest(&stopped, 1, true).unwrap().map(|r| r.manifest.len()),
            Some(1),
            "today's files after `STALE_S`"
        );
        let a = lead("development");
        let t1 = term::bootstrap(&stopped, &a, DAY).unwrap().unwrap();
        let taken = up(&stopped, &t1, None, &any).unwrap();
        assert_eq!((taken.records.seq, taken.records.manifest.len()), (2, 1));
        // Today's files, written after it, for today's readers.
        let m: BTreeMap<String, String> = serde_json::from_slice(&nas.read(TODAY[0]).unwrap().unwrap()).unwrap();
        assert_eq!(m, r.manifest);
        let k: Keys = serde_json::from_slice(&nas.read(TODAY[1]).unwrap().unwrap()).unwrap();
        assert_eq!(k.unit.get("6-1-2").map(String::as_str), Some("k1"));
        assert!(nas.read(TODAY[2]).unwrap().is_some());
    }

    #[test]
    fn a_merge_applies_in_lease_order_and_refuses_what_its_check_refuses() {
        let nas = Mem::default();
        let mut r = Records { term: 3, ..Default::default() };
        // Told in another order than leased: the later lease's key wins.
        let (a, b) = (built(3, 9, "6-1-1", "k9"), built(3, 2, "6-1-1", "k2"));
        let bad = Entry { step: "bogus".into(), ..built(3, 5, "6-1-3", "kx") };
        let keys: Vec<String> = [&a, &b, &bad].iter().map(|e| journal::write(&nas, e).unwrap()).collect();
        let short = "2026-10-06/3-7".to_string();
        nas.create_new(&journal::path(&short), b"").unwrap();
        let check = |e: &Entry, _: &Records| {
            if e.step == "bogus" {
                Err("writes outside its step's names".to_string())
            } else {
                Ok(())
            }
        };
        let mut told = keys.clone();
        told.push(short.clone());
        told.push(keys[0].clone());
        let m = merge(&nas, &mut r, &told, &check);
        assert_eq!(m.applied, [b.key().unwrap(), a.key().unwrap()]);
        assert_eq!(m.refused, [(bad.key().unwrap(), "writes outside its step's names".to_string())]);
        assert_eq!(m.waiting, [short]);
        assert_eq!(r.keys.unit.get("6-1-1").map(String::as_str), Some("k9"));
        assert_eq!(r.manifest.get("base/6-1-1").map(String::as_str), Some("base/6-1-1.k9.base"));
        assert!(!r.manifest.contains_key("base/6-1-3") && r.handles(&bad.key().unwrap()) && r.pending.len() == 2);
        // Told again: nothing more.
        assert_eq!(merge(&nas, &mut r, &keys, &check), Merged::default());
    }

    #[test]
    fn an_older_lease_merged_after_a_newer_one_is_passed_over() {
        // (Review M5: lease order held only within one merge, and an older lease's entry merged
        // later won.)
        let nas = Mem::default();
        let mut r = Records { term: 3, ..Default::default() };
        let newer = journal::write(&nas, &built(3, 9, "6-1-1", "k9")).unwrap();
        let older = journal::write(&nas, &built(3, 5, "6-1-1", "k5")).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&newer), &any).applied, std::slice::from_ref(&newer));
        let m = merge(&nas, &mut r, std::slice::from_ref(&older), &any);
        assert_eq!((m.applied.len(), m.overtaken.clone()), (0, vec![older.clone()]));
        assert_eq!(r.keys.unit.get("6-1-1").map(String::as_str), Some("k9"), "the newer lease's key");
        assert_eq!(r.manifest.get("base/6-1-1").map(String::as_str), Some("base/6-1-1.k9.base"));
        assert!(r.handles(&older), "named: it doesn't wait");
        // An older lease's entry for other targets, and a later prune of the unit, apply; a build
        // older than the prune doesn't bring it back.
        let other = journal::write(&nas, &built(3, 4, "6-1-2", "k4")).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&other), &any).applied, [other]);
        let h = Handoff {
            changes: [("base/6-1-1".to_string(), None)].into(),
            done: Some(("prune".into(), vec![("unit 6-1-1".into(), String::new())])),
            ..Default::default()
        };
        let prune =
            journal::write(&nas, &Entry { step: "prune".into(), handoff: h, ..built(3, 12, "6-1-1", "-") }).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&prune), &any).applied, [prune]);
        let late = journal::write(&nas, &built(3, 10, "6-1-1", "k10")).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&late), &any).overtaken, [late]);
        assert!(!r.manifest.contains_key("base/6-1-1") && !r.keys.unit.contains_key("6-1-1"));
        assert_eq!(r.last["unit"]["6-1-1"], LeaseId { term: 3, n: 12 });
        // Kept in the snapshot, for the next lead.
        assert_eq!(serde_json::from_slice::<Records>(&serde_json::to_vec(&r).unwrap()).unwrap(), r);
    }

    /// Task #780: a redo's entry (`scenic redo`: a job of the lead's, no file changed) passes the
    /// lead's check and forgets the key of the target it names alone, the manifest as it was; a
    /// build leased before it, merged after, is passed over, and one leased after applies; the
    /// same redo again changes nothing.
    #[test]
    fn a_redo_forgets_its_targets_keys_alone() {
        let nas = Mem::default();
        let mut r = Records { term: 3, ..Default::default() };
        let check = |e: &Entry, r: &Records| crate::agent::pool::check(crate::agent::seam::for_tests(), e, r);
        let keys: Vec<String> = [built(3, 1, "6-1-1", "k1"), built(3, 2, "6-1-2", "k2")]
            .iter()
            .map(|e| journal::write(&nas, e).unwrap())
            .collect();
        assert_eq!(merge(&nas, &mut r, &keys, &any).applied.len(), 2);
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
        let k = journal::write(&nas, &redo(5)).unwrap();
        let (ts, marks) = (["6-1-1".to_string()], crate::agent::redo::marks(&r, "unit", &["6-1-1".to_string()]));
        let m = merge(&nas, &mut r, std::slice::from_ref(&k), &check);
        // (#789: what `scenic redo --run` follows: the redo's lease is the target's mark now.)
        assert_ne!(crate::agent::redo::marks(&r, "unit", &ts), marks);
        assert_eq!(r.last["unit"]["6-1-1"], LeaseId { term: 3, n: 5 });
        assert!(m.refused.is_empty() && m.applied == [k], "{:?}", m.refused);
        assert_eq!(r.keys.unit.get("6-1-1"), None);
        assert_eq!(r.keys.unit.get("6-1-2").map(String::as_str), Some("k2"), "the other target's kept");
        assert_eq!(r.manifest, manifest, "no file changed");
        let late = journal::write(&nas, &built(3, 4, "6-1-1", "k4")).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&late), &any).overtaken, [late]);
        assert_eq!(r.keys.unit.get("6-1-1"), None, "a build leased before the redo doesn't bring its key back");
        let again = r.keys.clone();
        let k = journal::write(&nas, &redo(6)).unwrap();
        merge(&nas, &mut r, std::slice::from_ref(&k), &check);
        assert_eq!(r.keys, again, "a second redo changes nothing");
        // (Review: a redo's slots are its step's alone, under its old names too.)
        assert_eq!(slots("redo", "pois 6/1/1"), [("pois".to_string(), "6/1/1".to_string())]);
        assert_eq!(
            slots("redo", "terrain-hi 6/1/1"),
            [("terrain-hi".to_string(), "6/1/1".to_string()), ("terrain".into(), "6/1/1".into())]
        );
        let next = journal::write(&nas, &built(3, 7, "6-1-1", "k1")).unwrap();
        assert_eq!(merge(&nas, &mut r, std::slice::from_ref(&next), &any).applied, [next]);
        assert_eq!(r.keys, before, "built again under its key");
    }

    #[test]
    fn a_merge_reads_while_its_budget_lasts_and_leaves_the_rest() {
        // (Review 2: a lead taking up from an old snapshot read every entry since in one loop, on a
        // share under load an hour of reads.)
        let nas = Mem::default();
        let mut r = Records { term: 3, ..Default::default() };
        let mut keys: Vec<String> = [(3, 9, "6-1-1"), (2, 4, "6-1-2"), (3, 1, "6-1-3")]
            .iter()
            .map(|&(t, n, u)| journal::write(&nas, &built(t, n, u, "k")).unwrap())
            .collect();
        by_lease(&mut keys);
        assert_eq!(keys, ["2026-10-06/2-4", "2026-10-06/3-1", "2026-10-06/3-9"]);
        let reads = Cell::new(0);
        let m = merge_while(&nas, &mut r, &keys, &any, &|| {
            reads.set(reads.get() + 1);
            reads.get() <= 2
        });
        assert_eq!((m.applied, m.unread.clone()), (keys[..2].to_vec(), keys[2..].to_vec()));
        assert_eq!(merge(&nas, &mut r, &m.unread, &any).applied, keys[2..]);
    }

    #[test]
    fn a_read_that_fails_is_told_from_one_not_whole_and_the_rest_read() {
        // (A share that doesn't answer says nothing of an entry: a lead refuses one only once its
        // reads answer that it isn't whole, for an hour: driver::UNREADABLE_S.)
        struct Failing(Mem, String);
        impl Nas for Failing {
            fn create_new(&self, p: &str, bytes: &[u8]) -> Result<Created> {
                self.0.create_new(p, bytes)
            }
            fn write_whole(&self, p: &str, bytes: &[u8]) -> Result<()> {
                self.0.write_whole(p, bytes)
            }
            fn read(&self, p: &str) -> Result<Option<Vec<u8>>> {
                if p == self.1 {
                    bail!("read {p}: the share didn't answer");
                }
                self.0.read(p)
            }
            fn exists(&self, p: &str) -> Result<bool> {
                self.0.exists(p)
            }
            fn list(&self, dir: &str) -> Result<Vec<String>> {
                self.0.list(dir)
            }
            fn remove(&self, p: &str) -> Result<()> {
                self.0.remove(p)
            }
        }
        let mem = Mem::default();
        let keys: Vec<String> = [(3, 1, "6-1-1"), (3, 2, "6-1-2")]
            .iter()
            .map(|&(t, n, u)| journal::write(&mem, &built(t, n, u, "k")).unwrap())
            .collect();
        let short = "2026-10-06/3-3".to_string();
        mem.write_whole(&journal::path(&short), b"{\"member\":").unwrap();
        let nas = Failing(mem, journal::path(&keys[0]));
        let mut r = Records { term: 3, ..Default::default() };
        let m = merge(&nas, &mut r, &[keys.clone(), vec![short.clone()]].concat(), &any);
        assert_eq!(
            (m.failed, m.unread, m.applied, m.waiting),
            (vec![keys[0].clone()], vec![], vec![keys[1].clone()], vec![short])
        );
    }

    #[test]
    fn a_new_lead_takes_up_from_the_term_before_and_replays_what_it_lacks() {
        let nas = Mem::default();
        let a = lead("20261005-2202-61eb22c");
        let t1 = term::bootstrap(&nas, &a, DAY).unwrap().unwrap();
        let up1 = up(&nas, &t1, None, &any).unwrap();
        assert_eq!((up1.records.term, up1.records.seq), (1, 2), "after its first snapshot");
        // Term 1's lead merges one entry and saves; another is written and never merged.
        let (e1, e2) = (built(1, 1, "6-1-1", "k1"), built(1, 2, "6-1-2", "k2"));
        let k1 = journal::write(&nas, &e1).unwrap();
        journal::write(&nas, &e2).unwrap();
        let mut r1 = up1.records;
        merge(&nas, &mut r1, &[k1], &any);
        save(&mut r1, &nas).unwrap();
        // Taken over: term 2 starts from term 1's snapshot and replays the other.
        let b =
            Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "20261005-2202-61eb22c".into() };
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1.clone()) }, &b, "taken over", DAY + 60)
            .unwrap()
            .unwrap();
        // A reader while term 2 has no snapshot yet: term 1's.
        assert_eq!(Records::newest(&nas, 2, false).unwrap().map(|r| (r.term, r.seq)), Some((1, 3)));
        let up2 = up(&nas, &t2, None, &any).unwrap();
        assert_eq!(up2.merged.applied, [e2.key().unwrap()]);
        assert_eq!((up2.records.term, up2.records.seq), (2, 1));
        assert!(up2.records.handles(&e1.key().unwrap()) && up2.records.handles(&e2.key().unwrap()));
        assert_eq!(Records::load(&nas, 2).unwrap(), Some(up2.records.clone()));
        assert_eq!(Records::newest(&nas, 2, false).unwrap(), Some(up2.records.clone()));
        // Term 3 made, its lead gone before saving anything: term 4 starts from term 2's.
        let t3 = term::claim(&nas, &Current { term: 2, lead: Some(t2.clone()) }, &a, "taken over", DAY + 120)
            .unwrap()
            .unwrap();
        let t4 = term::claim(&nas, &Current { term: 3, lead: Some(t3) }, &b, "taken over", DAY + 900).unwrap().unwrap();
        assert_eq!(
            Records::newest(&nas, 4, false).unwrap().map(|r| r.term),
            Some(2),
            "readers meanwhile: term 2's (review L6: none)"
        );
        let up4 = up(&nas, &t4, None, &any).unwrap();
        assert_eq!((up4.records.term, up4.records.reflected.len()), (4, 2));
        // Tried again after its save landed: from its own snapshot, its number going on.
        let again = up(&nas, &t4, None, &any).unwrap();
        assert_eq!((again.records.term, again.records.seq), (4, 2));
    }

    #[test]
    fn readers_read_a_terms_brief_its_snapshot_without_one_and_todays_files_only_before_the_pool() {
        let nas = Mem::default();
        nas.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        // Before the pool: today's files.
        assert_eq!(Brief::now(&nas).unwrap().manifest.len(), 1);
        assert_eq!(Records::now(&nas).unwrap().manifest.len(), 1);
        let a = lead("20261005-2202-61eb22c");
        let t1 = term::bootstrap(&nas, &a, DAY).unwrap().unwrap();
        let mut r1 = up(&nas, &t1, None, &any).unwrap().records;
        assert!(nas.read(&brief_path(1)).unwrap().is_none(), "term 1's: today's files");
        let e1 = built(1, 1, "6-1-2", "k1");
        r1.apply(&e1.key().unwrap(), &e1);
        save(&mut r1, &nas).unwrap();
        let b1 = Brief::now(&nas).unwrap();
        assert_eq!((b1.term, b1.seq, b1.keys.unit.get("6-1-2").map(String::as_str)), (1, r1.seq, Some("k1")));
        // Term 2, its lead's snapshots each with its brief: one read, of a quarter the bytes.
        let b = Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: a.app.clone() };
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1.clone()) }, &b, "taken over", DAY + 60)
            .unwrap()
            .unwrap();
        let mut r2 = up(&nas, &t2, None, &any).unwrap().records;
        let e2 = built(2, 1, "6-1-3", "k2");
        r2.apply(&e2.key().unwrap(), &e2);
        save(&mut r2, &nas).unwrap();
        let brief = Brief::now(&nas).unwrap();
        assert_eq!(
            (brief.term, brief.seq, &brief.manifest, &brief.keys),
            (2, r2.seq, &r2.manifest, &r2.keys),
            "the term's brief, of its last snapshot"
        );
        assert!(nas.read(&brief_path(2)).unwrap().is_some());
        assert_eq!(Records::now(&nas).unwrap(), r2);
        // A lead on an app from before briefs (its snapshot alone): its snapshot.
        nas.remove(&brief_path(2)).unwrap();
        assert_eq!(Brief::now(&nas).unwrap(), Brief::of(r2.clone()));
        // Term 3 made, no snapshot yet: term 2's.
        term::claim(&nas, &Current { term: 2, lead: Some(t2) }, &a, "taken over", DAY + 120).unwrap().unwrap();
        assert_eq!(Brief::now(&nas).unwrap().term, 2);
        // No term found (the hint, the listing and the terms unreadable: stale) once the pool has
        // begun: the one path's, else an error,
        // never today's files (GC's live set from days-old records).
        struct Lost<'a>(&'a Mem, bool);
        impl Nas for Lost<'_> {
            fn create_new(&self, p: &str, b: &[u8]) -> Result<Created> {
                self.0.create_new(p, b)
            }
            fn write_whole(&self, p: &str, b: &[u8]) -> Result<()> {
                self.0.write_whole(p, b)
            }
            fn read(&self, p: &str) -> Result<Option<Vec<u8>>> {
                if p == term::HINT || (self.1 && (p == ONE || p == ONE_BRIEF)) {
                    Ok(None)
                } else {
                    self.0.read(p)
                }
            }
            fn exists(&self, p: &str) -> Result<bool> {
                Ok(!p.starts_with(term::DIR) && self.0.exists(p)?)
            }
            fn list(&self, _: &str) -> Result<Vec<String>> {
                Ok(Vec::new())
            }
            fn remove(&self, p: &str) -> Result<()> {
                self.0.remove(p)
            }
        }
        assert_eq!(Brief::now(&Lost(&nas, false)).unwrap().seq, r2.seq, "the one path's");
        assert_eq!(Records::now(&Lost(&nas, false)).unwrap(), r2);
        assert!(Brief::now(&Lost(&nas, true)).is_err());
        assert!(Records::now(&Lost(&nas, true)).is_err());
    }

    #[test]
    fn a_lead_re_asserting_starts_from_its_own_records_unread() {
        let nas = Mem::default();
        let a = lead("20261005-2202-61eb22c");
        let t1 = term::bootstrap(&nas, &a, DAY).unwrap().unwrap();
        let r = up(&nas, &t1, None, &any).unwrap().records;
        let mut own = r.clone();
        let e4 = built(1, 4, "6-1-4", "k4");
        own.apply(&e4.key().unwrap(), &e4);
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1) }, &a, "re-asserted after a gap", DAY + 900)
            .unwrap()
            .unwrap();
        let up2 = up(&nas, &t2, Some(own), &any).unwrap();
        assert_eq!(up2.records.keys.unit.get("6-1-4").map(String::as_str), Some("k4"));
        assert_eq!((up2.records.term, up2.records.seq), (2, 1));
    }

    #[test]
    fn a_refused_entrys_why_is_noted_once_its_refusal_is_saved() {
        let nas = Mem::default();
        let a = lead("development");
        let t1 = term::bootstrap(&nas, &a, DAY).unwrap().unwrap();
        let bad = Entry { step: "bogus".into(), ..built(1, 3, "6-1-3", "kx") };
        let k = journal::write(&nas, &bad).unwrap();
        let check = |e: &Entry, _: &Records| if e.step == "bogus" { Err("no such step".to_string()) } else { Ok(()) };
        let taken = up(&nas, &t1, None, &check).unwrap();
        assert_eq!(taken.records.rejected.get(&k).map(String::as_str), Some("no such step"));
        assert_eq!(journal::refusal(&nas, &k).unwrap().as_deref(), Some("no such step"));
        // Told again by its member, to a lead whose records lack it: checked again, refused again.
        let mut r = Records { term: 2, ..Default::default() };
        let m = merge(&nas, &mut r, std::slice::from_ref(&k), &check);
        assert_eq!(m.refused, [(k, "no such step".to_string())]);
    }

    #[test]
    fn a_stale_leads_refusal_is_checked_again_by_the_current_lead() {
        // A stale lead's check refuses an entry (say, it leased its targets again after the lease
        // lapsed, or its app's write-sets are older) and sets it aside. (Review M1: the current
        // lead, whose check would take it, refused it unread.)
        let nas = Mem::default();
        let (a, b) = (
            lead("development"),
            Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "development".into() },
        );
        let t1 = term::bootstrap(&nas, &a, DAY).unwrap().unwrap();
        let mut r1 = up(&nas, &t1, None, &any).unwrap().records;
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1) }, &b, "taken over by MacBook-Air", DAY + 700)
            .unwrap()
            .unwrap();
        let mut r2 = up(&nas, &t2, None, &any).unwrap().records;
        let x = journal::write(&nas, &built(1, 5, "6-1-1", "k5")).unwrap();
        let stale = |_: &Entry, _: &Records| Err::<(), String>("6-1-1 leased again since".into());
        let m = merge(&nas, &mut r1, std::slice::from_ref(&x), &stale);
        save(&mut r1, &nas).unwrap();
        for (k, why) in &m.refused {
            journal::note_refusal(&nas, k, why).unwrap();
        }
        // Told of it by its member, or (its member gone) listing it: its own check decides.
        assert_eq!(journal::list(&nas, None).unwrap(), std::slice::from_ref(&x));
        let m2 = merge(&nas, &mut r2.clone(), std::slice::from_ref(&x), &any);
        assert_eq!(m2.applied, std::slice::from_ref(&x), "{m2:?}");
        let m3 = merge(&nas, &mut r2, &journal::list(&nas, None).unwrap(), &any);
        assert_eq!(m3.applied, std::slice::from_ref(&x), "{m3:?}");
        assert_eq!(r2.keys.unit.get("6-1-1").map(String::as_str), Some("k5"));
    }

    /// A NAS on which term 2's first snapshot's save lands but answers an error, and reads it as
    /// missing a moment after (a stale read).
    #[derive(Default)]
    struct LostSave {
        mem: Mem,
        lost: Cell<bool>,
        stale: Cell<bool>,
    }

    impl Nas for LostSave {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
            self.mem.create_new(path, bytes)
        }
        fn write_whole(&self, p: &str, bytes: &[u8]) -> Result<()> {
            self.mem.write_whole(p, bytes)?;
            if p == path(2) && self.lost.replace(false) {
                self.stale.set(true);
                bail!("write {p}: the answer was lost");
            }
            Ok(())
        }
        fn read(&self, p: &str) -> Result<Option<Vec<u8>>> {
            if p == path(2) && self.stale.replace(false) {
                return Ok(None);
            }
            self.mem.read(p)
        }
        fn exists(&self, p: &str) -> Result<bool> {
            self.mem.exists(p)
        }
        fn list(&self, dir: &str) -> Result<Vec<String>> {
            self.mem.list(dir)
        }
        fn remove(&self, p: &str) -> Result<()> {
            self.mem.remove(p)
        }
    }

    #[test]
    fn a_take_up_tried_again_numbers_its_snapshot_on() {
        // Term 2's first save lands and answers an error; the next try reads it as missing.
        // (Review L1: the try started again from term 1's records at 0, and two versions of term
        // 2's records, with other entries, were both numbered 1.)
        let nas = LostSave::default();
        let (a, b) = (
            lead("development"),
            Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "development".into() },
        );
        let t1 = term::bootstrap(&nas, &a, DAY).unwrap().unwrap();
        up(&nas, &t1, None, &any).unwrap();
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1) }, &b, "taken over by MacBook-Air", DAY + 700)
            .unwrap()
            .unwrap();
        journal::write(&nas, &built(1, 1, "6-1-1", "k1")).unwrap();
        nas.lost.set(true);
        let mut own = None;
        assert!(take_up(&nas, &t2, &mut own, &journal::list(&nas, None).unwrap(), &any, false).is_err());
        let first = Records::load(&nas.mem, 2).unwrap().unwrap();
        assert_eq!(own.as_ref().map(|r| (r.term, r.seq)), Some((2, 1)), "kept as tried");
        journal::write(&nas, &built(1, 2, "6-1-2", "k2")).unwrap();
        let again = take_up(&nas, &t2, &mut own, &journal::list(&nas, None).unwrap(), &any, false).unwrap().records;
        assert_eq!((first.seq, again.seq, first.reflected.len(), again.reflected.len()), (1, 2, 1, 2));
        assert!(own.is_none());
    }

    #[test]
    fn term_ones_first_snapshot_whose_create_is_cut_is_written_whole() {
        // Its create made the file and its bytes didn't land. (Re-review 2, H1: term 1 was made all
        // the same, and a take-up under stale reads paired today's files of two versions.)
        struct Cut(Mem);
        impl Nas for Cut {
            fn create_new(&self, p: &str, _: &[u8]) -> Result<Created> {
                self.0.create_new(p, b"")?;
                Ok(Created::Unwritten(anyhow::anyhow!("write {p} (made, and left short)")))
            }
            fn write_whole(&self, p: &str, bytes: &[u8]) -> Result<()> {
                self.0.write_whole(p, bytes)
            }
            fn read(&self, p: &str) -> Result<Option<Vec<u8>>> {
                self.0.read(p)
            }
            fn exists(&self, p: &str) -> Result<bool> {
                self.0.exists(p)
            }
            fn list(&self, dir: &str) -> Result<Vec<String>> {
                self.0.list(dir)
            }
            fn remove(&self, p: &str) -> Result<()> {
                self.0.remove(p)
            }
        }
        let nas = Cut(Mem::default());
        nas.0.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        first(&nas).unwrap();
        assert_eq!(Records::newest(&nas, 1, false).unwrap().map(|r| (r.seq, r.manifest.len())), Some((1, 1)));
    }

    #[test]
    fn term_ones_first_snapshot_finished_late_lands_where_none_reads() {
        // Its maker's create cut short, and its finish landing late, its Mac asleep between its
        // temporary file and its rename: term 1's lead took up from today's files meanwhile, and
        // saved. (Re-review 3: written to term 1's records file, it landed over that snapshot.)
        let nas = Mem::default();
        nas.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        let late = serde_json::to_vec(&Records { seq: 1, ..Records::today(&nas).unwrap() }).unwrap();
        nas.create_new(FIRST, b"").unwrap();
        let t1 = term::bootstrap(&nas, &lead("development"), DAY).unwrap().unwrap();
        let mut r = up(&nas, &t1, None, &any).unwrap().records;
        let e = built(1, 1, "6-1-2", "k1");
        r.apply(&e.key().unwrap(), &e);
        save(&mut r, &nas).unwrap();
        nas.write_whole(FIRST, &late).unwrap();
        assert_eq!(Records::newest(&nas, 1, false).unwrap(), Some(r.clone()), "its lead's");
        let b = Member { id: "m-000000000000000b".into(), host: "MacBook-Air".into(), app: "development".into() };
        let t2 = term::claim(&nas, &Current { term: 1, lead: Some(t1) }, &b, "taken over", DAY + 60).unwrap().unwrap();
        assert!(start(&nas, &t2, None, false, None).unwrap().handles(&e.key().unwrap()), "term 2 from its lead's too");
    }

    #[test]
    fn term_ones_first_snapshot_with_a_hole_is_taken_up_from_todays_files() {
        // Its bytes landed out of order and its maker stopped: a hole of zeros mid-file, not a
        // short end. (Review L5: it couldn't be read, nor term 1 taken up.)
        let nas = Mem::default();
        nas.write_whole(TODAY[0], br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        let mut b = serde_json::to_vec(&Records { seq: 1, ..Records::today(&nas).unwrap() }).unwrap();
        let n = b.len();
        b[n / 4..n / 2].fill(0);
        nas.create_new(FIRST, &b).unwrap();
        let a = lead("development");
        let t1 = term::bootstrap(&nas, &a, DAY).unwrap().unwrap();
        let taken = up(&nas, &t1, None, &any).unwrap();
        assert_eq!((taken.records.seq, taken.records.manifest.len()), (2, 1));
        assert_eq!(Records::load(&nas, 1).unwrap().map(|r| r.seq), Some(2), "its lead's, saved whole");
    }
}
