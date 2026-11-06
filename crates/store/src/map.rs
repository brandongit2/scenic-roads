//! The map and its log (docs/maplog.md §4): the current name → file map as flat entries, the
//! batches that change it, its multiset digest, and the whole map as a snapshot (the lead's own and
//! the hourly one on the NAS, `map/<seq>.json.zst`). The log store on the lead's disk is `log`.
//!
//! Readers don't see entries: `Map::catalogue` gives today's `crate::catalogue::Catalogue` (`n` the
//! map's `seq`, `created` its batch's time), and `entries` turns a catalogue into entries, the two
//! exact inverses (tests: on test catalogues and a recorded chain of real ones).

use crate::catalogue::{Catalogue, FileRef, Layer};
use crate::nasfs::NasRoot;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;

pub mod feed;
pub mod gc;
pub mod log;
mod val;

pub use val::Val;

/// The map's entries, key → value (docs/maplog.md §4.1), each value its canonical JSON (`Val`).
pub type Entries = BTreeMap<String, Val>;

/// A layer's sections kept as entries of their own (`l/<layer>/<section>/<tile>`), not in its head.
const LAYER_SECTIONS: [&str; 5] = ["lo", "hi", "deep", "native_zoom", "polar"];

/// The NAS folder of the map's snapshots (§4.4) and how many of the newest are kept.
pub const NAS_DIR: &str = "map";
pub const NAS_KEPT: usize = 3;
const SUFFIX: &str = ".json.zst";

/// One entry's hash: XXH3-128 over its key, a zero byte and its value's canonical JSON (`Val`:
/// serde_json's maps are sorted, so the same value, the same bytes).
pub fn entry_hash(key: &str, v: &Val) -> u128 {
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    h.update(key.as_bytes());
    h.update(&[0]);
    h.update(v.json().as_bytes());
    h.digest128()
}

/// The digest of a whole set of entries, from scratch: the sum of their hashes modulo 2^128 (§4.3).
pub fn digest_of(entries: &Entries) -> u128 {
    entries.iter().fold(0u128, |s, (k, v)| s.wrapping_add(entry_hash(k, v)))
}

/// A digest as written: 32 hex digits.
pub fn hex(d: u128) -> String {
    format!("{d:032x}")
}

fn unhex(s: &str) -> Result<u128> {
    u128::from_str_radix(s, 16).with_context(|| format!("{s:?} isn't a digest"))
}

/// A catalogue as entries (§4.1). An error for a catalogue whose parts can't be told apart once
/// flattened (a layer's name with a `/`, a key made twice).
pub fn entries(cat: &Catalogue) -> Result<Entries> {
    let mut e = Entries::new();
    each_entry(cat, &mut |k, v| {
        ensure!(e.insert(k.to_string(), v).is_none(), "entry {k:?} made twice");
        Ok(())
    })?;
    Ok(e)
}

/// `entries` one at a time, never all at once (each made, handed to `f`, dropped): how the lead
/// diffs a catalogue against the map while holding the map once. In no particular order.
pub fn each_entry(cat: &Catalogue, f: &mut dyn FnMut(&str, Val) -> Result<()>) -> Result<()> {
    for (l, r) in &cat.files {
        f(&format!("f/{l}"), serde_json::to_value(r)?.into())?;
    }
    for (name, layer) in &cat.layers {
        ensure!(!name.contains('/') && !name.is_empty(), "layer {name:?}: a layer's name can't be empty or hold a /");
        // (Its head: the layer less its sections, which are never serialized only to be dropped.)
        let head = Layer {
            encoding: layer.encoding.clone(),
            minzoom: layer.minzoom,
            maxzoom: layer.maxzoom,
            root: layer.root.clone(),
            extra: layer.extra.clone(),
            ..Default::default()
        };
        let mut head = serde_json::to_value(&head)?;
        let o = head.as_object_mut().context("a layer is an object")?;
        for s in LAYER_SECTIONS {
            o.remove(s);
        }
        f(&format!("l/{name}"), head.into())?;
        for (s, m) in [("lo", &layer.lo), ("hi", &layer.hi), ("deep", &layer.deep), ("polar", &layer.polar)] {
            for (t, v) in m {
                f(&format!("l/{name}/{s}/{t}"), Value::from(v.as_str()).into())?;
            }
        }
        for (t, z) in &layer.native_zoom {
            f(&format!("l/{name}/native_zoom/{t}"), Value::from(*z).into())?;
        }
    }
    for (s, m) in sections(cat) {
        for (k, v) in m {
            f(&format!("s/{s}/{k}"), Value::from(v.as_str()).into())?;
        }
    }
    // (Sorted and each once, as catalogues write them: one entry each; else kept whole.)
    if cat.units.windows(2).all(|w| w[0] < w[1]) {
        for u in &cat.units {
            f(&format!("u/{u}"), Value::Bool(true).into())?;
        }
    } else {
        f("m/units", serde_json::to_value(&cat.units)?.into())?;
    }
    f("m/fmt", Value::from(cat.fmt).into())?;
    if !cat.app.is_empty() {
        f("m/app", Value::from(cat.app.as_str()).into())?;
    }
    if !cat.basemap.is_empty() {
        f("m/basemap", serde_json::to_value(&cat.basemap)?.into())?;
    }
    for (k, v) in [("meta", &cat.meta), ("credits", &cat.credits)] {
        if !v.is_null() {
            f(&format!("m/{k}"), v.into())?;
        }
    }
    if !cat.dem_labels.is_empty() {
        f("m/dem_labels", serde_json::to_value(&cat.dem_labels)?.into())?;
    }
    coverage_entries(&cat.coverage, f)?;
    for (k, v) in &cat.extra {
        f(&format!("m/extra/{k}"), v.into())?;
    }
    Ok(())
}

/// The catalogue's maps of one logical name by key, by their entries' section name.
fn sections(cat: &Catalogue) -> [(&'static str, &BTreeMap<String, String>); 6] {
    [
        ("base", &cat.base),
        ("roads", &cat.roads),
        ("hidata", &cat.hidata),
        ("markdata", &cat.markdata),
        ("ovdata", &cat.ovdata),
        ("global", &cat.global),
    ]
}

/// The coverage: each region an entry `r/<id>` with its place in the order (`{"i", "region"}`), and
/// the rest of it in `m/coverage`, `m/regions` saying how many regions there are. A coverage whose
/// regions aren't a list of objects with distinct ids stays whole in `m/coverage`.
fn coverage_entries(cov: &Value, f: &mut dyn FnMut(&str, Val) -> Result<()>) -> Result<()> {
    if cov.is_null() {
        return Ok(());
    }
    let split = cov.get("regions").and_then(Value::as_array).filter(|rs| {
        let ids: std::collections::BTreeSet<&str> =
            rs.iter().filter_map(|r| r.get("id").and_then(Value::as_str)).collect();
        ids.len() == rs.len()
    });
    let (Some(rs), Some(o)) = (split, cov.as_object()) else {
        return f("m/coverage", cov.into());
    };
    for (i, r) in rs.iter().enumerate() {
        let id = r.get("id").and_then(Value::as_str).expect("checked above");
        f(&format!("r/{id}"), Val::region(i, r))?;
    }
    let rest = o.iter().filter(|(k, _)| *k != "regions").map(|(k, v)| (k.clone(), v.clone())).collect();
    f("m/coverage", Value::Object(rest).into())?;
    f("m/regions", Value::from(rs.len()).into())
}

/// The catalogue entries make, numbered `n` and made at `created` (§4.1): `entries`' inverse.
pub fn catalogue(e: &Entries, n: u64, created: &str) -> Result<Catalogue> {
    let mut c = Catalogue { n, created: created.to_string(), ..Default::default() };
    let mut layer_parts: Vec<(&str, &str, &str, &Val)> = Vec::new();
    let mut regions: Vec<(u64, Value)> = Vec::new();
    let (mut cov, mut n_regions) = (None, None);
    let s = |v: &Val, k: &str| v.as_str().map(String::from).with_context(|| format!("{k}: not a name"));
    for (k, v) in e {
        let (kind, rest) = k.split_once('/').with_context(|| format!("entry {k:?} has no kind"))?;
        match kind {
            "f" => {
                c.files.insert(rest.into(), v.parse::<FileRef>().with_context(|| k.clone())?);
            }
            "l" => match rest.split_once('/') {
                None => {
                    c.layers.insert(rest.into(), v.parse::<Layer>().with_context(|| k.clone())?);
                }
                Some((name, sec)) => {
                    let (sec, tile) = sec.split_once('/').with_context(|| format!("entry {k:?}: no tile"))?;
                    layer_parts.push((name, sec, tile, v));
                }
            },
            "s" => {
                let (sec, key) = rest.split_once('/').with_context(|| format!("entry {k:?}: no key"))?;
                let m = match sec {
                    "base" => &mut c.base,
                    "roads" => &mut c.roads,
                    "hidata" => &mut c.hidata,
                    "markdata" => &mut c.markdata,
                    "ovdata" => &mut c.ovdata,
                    "global" => &mut c.global,
                    _ => bail!("entry {k:?}: no section {sec}"),
                };
                m.insert(key.into(), s(v, k)?);
            }
            "u" => c.units.push(rest.into()),
            "r" => {
                let mut r = v.value();
                let i = r.get("i").and_then(Value::as_u64).with_context(|| format!("entry {k:?}: no place"))?;
                regions.push((i, r.get_mut("region").map(Value::take).unwrap_or_default()));
            }
            "m" => match rest {
                "fmt" => c.fmt = v.parse().context("m/fmt")?,
                "app" => c.app = s(v, k)?,
                "basemap" => c.basemap = v.parse().context("m/basemap")?,
                "meta" => c.meta = v.value(),
                "credits" => c.credits = v.value(),
                "dem_labels" => c.dem_labels = v.parse().context("m/dem_labels")?,
                "units" => c.units = v.parse().context("m/units")?,
                "coverage" => cov = Some(v.value()),
                "regions" => n_regions = Some(v.parse::<u64>().context("m/regions")? as usize),
                _ => match rest.strip_prefix("extra/") {
                    Some(x) => {
                        c.extra.insert(x.into(), v.value());
                    }
                    None => bail!("entry {k:?}: no such field"),
                },
            },
            _ => bail!("entry {k:?}: no kind {kind}"),
        }
    }
    for (name, sec, tile, v) in layer_parts {
        let l = c.layers.get_mut(name).with_context(|| format!("layer {name}'s {sec} without its head"))?;
        let m = match sec {
            "lo" => &mut l.lo,
            "hi" => &mut l.hi,
            "deep" => &mut l.deep,
            "polar" => &mut l.polar,
            "native_zoom" => {
                let z = v.parse::<u8>().with_context(|| format!("{name}'s zoom of {tile}"))?;
                l.native_zoom.insert(tile.into(), z);
                continue;
            }
            _ => bail!("layer {name}: no section {sec}"),
        };
        m.insert(tile.into(), s(v, tile)?);
    }
    match n_regions {
        None => {
            ensure!(regions.is_empty(), "regions without their count");
            c.coverage = cov.unwrap_or(Value::Null);
        }
        Some(count) => {
            regions.sort_by_key(|r| r.0);
            ensure!(
                regions.len() == count && regions.iter().enumerate().all(|(i, r)| r.0 == i as u64),
                "{} regions, {count} counted, or their places aren't 0 to {count}",
                regions.len()
            );
            let mut o = match cov {
                Some(Value::Object(o)) => o,
                _ => bail!("regions without the coverage they're in"),
            };
            o.insert("regions".into(), Value::Array(regions.into_iter().map(|r| r.1).collect()));
            c.coverage = Value::Object(o);
        }
    }
    Ok(c)
}

/// The map at one `seq`: its entries, the time of the batch that made it and its digest, kept as
/// the entries change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map {
    pub seq: u64,
    pub at: String,
    entries: Entries,
    digest: u128,
}

/// One batch of the log (§4.2): the entries set and deleted, the content names no longer named
/// (`freed`), and the map's digest after it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    pub seq: u64,
    pub at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    #[serde(default)]
    pub set: Entries,
    #[serde(default)]
    pub del: Vec<String>,
    #[serde(default)]
    pub freed: Vec<String>,
    pub digest: String,
}

impl Batch {
    /// Whether it changes nothing (no batch is written for it).
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.del.is_empty()
    }

    /// Whether its `why` says `part`: its parts after the first, each after `WHY_SEP` ("catalogue
    /// 812; the low zooms").
    pub fn says(&self, part: &str) -> bool {
        self.why.split(WHY_SEP).skip(1).any(|p| p == part)
    }

    /// Whether it sets or deletes a key starting with `prefix`.
    pub fn touches(&self, prefix: &str) -> bool {
        let set = self.set.range::<str, _>((std::ops::Bound::Included(prefix), std::ops::Bound::Unbounded)).next();
        set.is_some_and(|(k, _)| k.starts_with(prefix)) || self.del.iter().any(|k| k.starts_with(prefix))
    }
}

/// What separates the parts of a batch's `why` (`Batch::says`).
pub const WHY_SEP: &str = "; ";

/// A watch of the log (`log::Log::open_watching`) is a key prefix (the last batch setting or deleting
/// a key under it), or this and a part of a batch's `why` (the last batch saying it).
pub const WHY: &str = "why:";

/// Whether batch `b` is one watch `w` marks.
pub fn marks(w: &str, b: &Batch) -> bool {
    match w.strip_prefix(WHY) {
        Some(part) => b.says(part),
        None => b.touches(w),
    }
}

/// The last batch a watch marked (docs/maplog.md §6.8): its `seq` and time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub seq: u64,
    pub at: String,
}

/// An `f/` entry's content name: its `file` when it's an object whose `file` is a string.
pub fn content_of(v: &Val) -> Option<String> {
    #[derive(Deserialize)]
    struct File {
        file: String,
    }
    // (Objects only: a derived struct also reads a list, `["x"]` as `file` "x".)
    v.json().starts_with('{').then(|| v.parse::<File>().ok().map(|f| f.file)).flatten()
}

impl Map {
    /// The map holding `entries` at `seq`, made at `at`.
    pub fn new(seq: u64, at: String, entries: Entries) -> Map {
        let digest = digest_of(&entries);
        Map { seq, at, entries, digest }
    }

    /// The map a catalogue is, at its number and time.
    pub fn of(cat: &Catalogue) -> Result<Map> {
        Ok(Map::new(cat.n, cat.created.clone(), entries(cat)?))
    }

    pub fn entries(&self) -> &Entries {
        &self.entries
    }

    pub fn digest(&self) -> u128 {
        self.digest
    }

    /// Whether it holds nothing (a log not started).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Today's catalogue of it (`n` its `seq`, `created` its time).
    pub fn catalogue(&self) -> Result<Catalogue> {
        catalogue(&self.entries, self.seq, &self.at)
    }

    /// The batch from this map to `next`, numbered `seq`, made at `at`.
    pub fn batch_to(&self, next: &Entries, seq: u64, at: &str, app: &str, why: &str) -> Batch {
        let source = |f: &mut dyn FnMut(&str, Val) -> Result<()>| next.iter().try_for_each(|(k, v)| f(k, v.clone()));
        self.diff(source, seq, at, app, why).expect("a map's entries are each once")
    }

    /// The batch from this map to catalogue `cat` (`seq` its number, `at` its time), its entries made
    /// one at a time (`each_entry`): the map and the batch held, never a second whole map.
    pub fn batch_of(&self, cat: &Catalogue, app: &str, why: &str) -> Result<Batch> {
        self.diff(|f| each_entry(cat, f), cat.n, &cat.created, app, why)
    }

    /// The batch from this map to the entries `source` hands its callback (each key once).
    fn diff(
        &self,
        source: impl FnOnce(&mut dyn FnMut(&str, Val) -> Result<()>) -> Result<()>,
        seq: u64,
        at: &str,
        app: &str,
        why: &str,
    ) -> Result<Batch> {
        let mut set = Entries::new();
        let mut d = self.digest;
        let mut freed = Vec::new();
        // (The map's keys `next` has, to find those it hasn't: pointers into the map, not copies.)
        let mut kept: std::collections::HashSet<&str> = std::collections::HashSet::with_capacity(self.entries.len());
        source(&mut |k, v| {
            let old = self.entries.get_key_value(k);
            if let Some((mk, o)) = old {
                ensure!(kept.insert(mk.as_str()), "entry {k:?} made twice");
                if *o == v {
                    return Ok(());
                }
                d = d.wrapping_sub(entry_hash(k, o));
                if let Some(f) = k.starts_with("f/").then(|| content_of(o)).flatten() {
                    if content_of(&v).as_ref() != Some(&f) {
                        freed.push(f);
                    }
                }
            }
            d = d.wrapping_add(entry_hash(k, &v));
            ensure!(set.insert(k.to_string(), v).is_none() || old.is_some(), "entry {k:?} made twice");
            Ok(())
        })?;
        let mut del = Vec::new();
        for (k, o) in self.entries.iter().filter(|(k, _)| !kept.contains(k.as_str())) {
            d = d.wrapping_sub(entry_hash(k, o));
            if let Some(f) = k.starts_with("f/").then(|| content_of(o)).flatten() {
                freed.push(f);
            }
            del.push(k.clone());
        }
        freed.sort();
        Ok(Batch { seq, at: at.into(), app: app.into(), why: why.into(), set, del, freed, digest: hex(d) })
    }

    /// Whether this map is catalogue `cat`, entry for entry, its kept digest the sum of `cat`'s
    /// entries' hashes from scratch: an error saying where they differ. One pass over `cat`'s
    /// entries made one at a time, nothing more held.
    pub fn check_is(&self, cat: &Catalogue) -> Result<()> {
        let (mut n, mut d) = (0usize, 0u128);
        each_entry(cat, &mut |k, v| {
            ensure!(self.entries.get(k) == Some(&v), "the map differs from the catalogue at {k}");
            d = d.wrapping_add(entry_hash(k, &v));
            n += 1;
            Ok(())
        })?;
        ensure!(n == self.entries.len(), "the map holds {} entries, the catalogue {n}", self.entries.len());
        ensure!(d == self.digest, "the map's digest is {}, its entries' {}", hex(self.digest), hex(d));
        Ok(())
    }

    /// The map after `b`, applied to a copy: this one is left as it was when `b` doesn't apply.
    pub fn apply(&self, b: &Batch) -> Result<Map> {
        let mut m = self.clone();
        drop(m.apply_in(b)?);
        Ok(m)
    }

    /// `b` applied to this map in place, or nothing changed when it doesn't apply (a `seq` not past
    /// this one's, an entry deleted that isn't there, or the digest after it not the batch's): what
    /// it replaced kept to put back (`undo`), the size of the batch, never a copy of the map.
    pub fn apply_in(&mut self, b: &Batch) -> Result<Undo> {
        ensure!(b.seq > self.seq, "batch {} isn't past the map's {}", b.seq, self.seq);
        let want = unhex(&b.digest)?;
        let mut u = Undo { seq: self.seq, at: std::mem::take(&mut self.at), digest: self.digest, was: Vec::new() };
        let r = self.apply_entries(b, want, &mut u.was);
        match r {
            Ok(()) => {
                self.seq = b.seq;
                self.at = b.at.clone();
                Ok(u)
            }
            Err(e) => {
                self.undo(u);
                Err(e)
            }
        }
    }

    fn apply_entries(&mut self, b: &Batch, want: u128, was: &mut Vec<(String, Option<Val>)>) -> Result<()> {
        for k in &b.del {
            let o =
                self.entries.remove(k).with_context(|| format!("batch {} deletes {k}, which isn't there", b.seq))?;
            self.digest = self.digest.wrapping_sub(entry_hash(k, &o));
            was.push((k.clone(), Some(o)));
        }
        for (k, v) in &b.set {
            let o = self.entries.insert(k.clone(), v.clone());
            if let Some(o) = &o {
                self.digest = self.digest.wrapping_sub(entry_hash(k, o));
            }
            self.digest = self.digest.wrapping_add(entry_hash(k, v));
            was.push((k.clone(), o));
        }
        ensure!(
            self.digest == want,
            "batch {}: the map after it has digest {}, the batch says {}",
            b.seq,
            hex(self.digest),
            b.digest
        );
        Ok(())
    }

    /// Puts back what `apply_in` changed.
    pub fn undo(&mut self, u: Undo) {
        for (k, o) in u.was.into_iter().rev() {
            match o {
                Some(o) => self.entries.insert(k, o),
                None => self.entries.remove(&k),
            };
        }
        (self.seq, self.at, self.digest) = (u.seq, u.at, u.digest);
    }

    /// Whether the digest kept equals the one of its entries from scratch.
    pub fn digest_holds(&self) -> bool {
        digest_of(&self.entries) == self.digest
    }
}

/// What `Map::apply_in` replaced, to put back.
#[must_use]
pub struct Undo {
    seq: u64,
    at: String,
    digest: u128,
    was: Vec<(String, Option<Val>)>,
}

/// A client's cursor (§5.4): the last `seq` it applied and when it was heard (seconds since 1970).
/// Saved with the log's snapshot (Decisions, 6).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    pub seq: u64,
    pub heard: u64,
    /// When it was first heard at `seq`: its adoption of it (docs/maplog.md §6.1).
    #[serde(default)]
    pub at: u64,
    /// A mirror's (§6.2): the map whose files its round copies, and the last one it was through
    /// at, each as first heard (`feed::Feed::said`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copying: Option<Heard>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synced: Option<Heard>,
}

/// A `seq` a client said, and when it first said it (seconds since 1970).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heard {
    pub seq: u64,
    pub at: u64,
}

/// The whole map as stored: the lead's snapshot and the NAS's (§4.4, §4.5). Written from the map
/// it borrows (`of`), never a copy of it.
#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot<E = Entries, C = BTreeMap<String, Cursor>> {
    pub seq: u64,
    pub at: String,
    pub digest: String,
    pub entries: E,
    #[serde(default)]
    pub cursors: C,
    /// The log's watches' marks (`log::Log::marks`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub marks: BTreeMap<String, Mark>,
}

impl<'a> Snapshot<&'a Entries, &'a BTreeMap<String, Cursor>> {
    pub fn of(m: &'a Map, cursors: &'a BTreeMap<String, Cursor>) -> Self {
        Snapshot {
            seq: m.seq,
            at: m.at.clone(),
            digest: hex(m.digest),
            entries: &m.entries,
            cursors,
            marks: BTreeMap::new(),
        }
    }

    /// Compact JSON streamed into zstd (level 3: written hourly, read at a start) with its content
    /// checksum: the compressed bytes held, never the JSON whole.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut enc = zstd::stream::Encoder::new(Vec::with_capacity(1 << 20), 3)?;
        enc.include_checksum(true)?;
        let mut w = std::io::BufWriter::with_capacity(1 << 16, &mut enc);
        serde_json::to_writer(&mut w, self)?;
        w.flush()?;
        drop(w);
        Ok(enc.finish()?)
    }
}

impl Snapshot {
    /// The map and cursors it holds: an error when it's damaged or its entries aren't its digest.
    pub fn decode(bytes: &[u8]) -> Result<(Map, BTreeMap<String, Cursor>)> {
        Snapshot::decode_marked(bytes).map(|(m, c, _)| (m, c))
    }

    /// `decode`, with the marks it holds.
    pub fn decode_marked(bytes: &[u8]) -> Result<(Map, BTreeMap<String, Cursor>, BTreeMap<String, Mark>)> {
        // (Streamed: the JSON never whole; the frame's checksum checked at its end.)
        let dec = zstd::stream::Decoder::new(bytes).context("the map's snapshot doesn't decompress")?;
        let s: Snapshot = serde_json::from_reader(std::io::BufReader::with_capacity(1 << 16, dec))
            .context("the map's snapshot doesn't decompress or parse")?;
        let m = Map::new(s.seq, s.at, s.entries);
        ensure!(hex(m.digest) == s.digest, "the map's snapshot {} isn't its digest", s.seq);
        Ok((m, s.cursors, s.marks))
    }
}

/// `map/<seq>.json.zst`'s name.
pub fn nas_name(seq: u64) -> String {
    format!("{seq}{SUFFIX}")
}

/// The NAS folder of the map the `catalog` step would publish while publishing is held for review
/// (`inputs/hold-catalog`): `map-held/<seq>.json.zst`, a snapshot of it whole, `seq` the batch's it
/// would be (docs/maplog.md §6.4). No server reads it; lifting the hold publishes the map then.
pub const HELD_DIR: &str = "map-held";

/// The `seq`s of the snapshots in `dir` (`map/`, `map-held/`), newest first.
pub fn seqs_in(dir: &crate::nasfs::NasPath) -> Result<Vec<u64>> {
    let mut seqs: Vec<u64> = match dir.list() {
        Ok(names) => names.iter().filter_map(|n| n.strip_suffix(SUFFIX)?.parse().ok()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
    };
    seqs.sort_unstable_by(|a, b| b.cmp(a));
    Ok(seqs)
}

/// The newest snapshot in `dir` (`map/` of a NAS or a test root, `map-held/`) that reads (one
/// damaged or half written is passed over, said); None when there's none.
pub fn newest_in(dir: &crate::nasfs::NasPath) -> Result<Option<Map>> {
    for seq in seqs_in(dir)? {
        let p = dir.join(nas_name(seq));
        match p.read().map_err(anyhow::Error::from).and_then(|b| Snapshot::decode(&b)) {
            Ok((m, _)) => return Ok(Some(m)),
            Err(e) => eprintln!("maplog: {} passed over: {e:#}", p.display()),
        }
    }
    Ok(None)
}

/// `newest_in` the NAS's `map/` (`root`'s).
pub fn newest_nas(root: &NasRoot) -> Result<Option<Map>> {
    newest_in(&root.join(NAS_DIR))
}

/// Writes the map whole to the NAS (`map/<seq>.json.zst`, a temporary name renamed into place: §4.4),
/// then removes all but the newest `NAS_KEPT` there. Once an hour at most (the lead's caller). The
/// oldest `seq` kept there: a reader may start from it (GC's `snapshot` holder, docs/maplog.md §5.4).
pub fn write_nas_snapshot(root: &NasRoot, m: &Map) -> Result<u64> {
    write_snapshot_in(root, NAS_DIR, m)
}

/// `write_nas_snapshot` into `root`'s folder `name` (`NAS_DIR`, `HELD_DIR`), the newest
/// `NAS_KEPT` kept there.
pub fn write_snapshot_in(root: &NasRoot, name: &str, m: &Map) -> Result<u64> {
    let dir = root.join(name);
    let b = Snapshot::of(m, &BTreeMap::new()).encode()?;
    dir.join(nas_name(m.seq)).write_whole_made(&b).with_context(|| format!("write {name}/{}", nas_name(m.seq)))?;
    let seqs = seqs_in(&dir)?;
    let oldest = seqs.iter().take(NAS_KEPT).min().copied().unwrap_or(m.seq).min(m.seq);
    for s in seqs.into_iter().skip(NAS_KEPT) {
        if let Err(e) = dir.join(nas_name(s)).remove_file() {
            eprintln!("maplog: can't remove {name}/{}: {e}", nas_name(s));
        }
    }
    Ok(oldest)
}

#[cfg(test)]
mod tests;
