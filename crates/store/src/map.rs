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

pub mod log;

/// The map's entries, key → value (docs/maplog.md §4.1).
pub type Entries = BTreeMap<String, Value>;

/// A layer's sections kept as entries of their own (`l/<layer>/<section>/<tile>`), not in its head.
const LAYER_SECTIONS: [&str; 5] = ["lo", "hi", "deep", "native_zoom", "polar"];

/// The NAS folder of the map's snapshots (§4.4) and how many of the newest are kept.
pub const NAS_DIR: &str = "map";
pub const NAS_KEPT: usize = 3;
const SUFFIX: &str = ".json.zst";

/// One entry's hash: XXH3-128 over its key, a zero byte and its value's canonical JSON (serde_json's
/// maps are sorted: the same value, the same bytes).
pub fn entry_hash(key: &str, v: &Value) -> u128 {
    let mut b = Vec::with_capacity(key.len() + 64);
    b.extend_from_slice(key.as_bytes());
    b.push(0);
    serde_json::to_writer(&mut b, v).expect("a JSON value always serializes");
    xxhash_rust::xxh3::xxh3_128(&b)
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
/// flattened (a layer's name with a `/`).
pub fn entries(cat: &Catalogue) -> Result<Entries> {
    let mut e = Entries::new();
    for (l, f) in &cat.files {
        e.insert(format!("f/{l}"), serde_json::to_value(f)?);
    }
    for (name, layer) in &cat.layers {
        ensure!(!name.contains('/') && !name.is_empty(), "layer {name:?}: a layer's name can't be empty or hold a /");
        let mut head = serde_json::to_value(layer)?;
        let o = head.as_object_mut().context("a layer is an object")?;
        for s in LAYER_SECTIONS {
            o.remove(s);
        }
        e.insert(format!("l/{name}"), head);
        for (s, m) in [("lo", &layer.lo), ("hi", &layer.hi), ("deep", &layer.deep), ("polar", &layer.polar)] {
            for (t, v) in m {
                e.insert(format!("l/{name}/{s}/{t}"), Value::from(v.as_str()));
            }
        }
        for (t, z) in &layer.native_zoom {
            e.insert(format!("l/{name}/native_zoom/{t}"), Value::from(*z));
        }
    }
    for (s, m) in sections(cat) {
        for (k, v) in m {
            e.insert(format!("s/{s}/{k}"), Value::from(v.as_str()));
        }
    }
    // (Sorted and each once, as catalogues write them: one entry each; else kept whole.)
    if cat.units.windows(2).all(|w| w[0] < w[1]) {
        for u in &cat.units {
            e.insert(format!("u/{u}"), Value::Bool(true));
        }
    } else {
        e.insert("m/units".into(), serde_json::to_value(&cat.units)?);
    }
    e.insert("m/fmt".into(), Value::from(cat.fmt));
    if !cat.app.is_empty() {
        e.insert("m/app".into(), Value::from(cat.app.as_str()));
    }
    if !cat.basemap.is_empty() {
        e.insert("m/basemap".into(), serde_json::to_value(&cat.basemap)?);
    }
    for (k, v) in [("meta", &cat.meta), ("credits", &cat.credits)] {
        if !v.is_null() {
            e.insert(format!("m/{k}"), v.clone());
        }
    }
    if !cat.dem_labels.is_empty() {
        e.insert("m/dem_labels".into(), serde_json::to_value(&cat.dem_labels)?);
    }
    coverage_entries(&cat.coverage, &mut e);
    for (k, v) in &cat.extra {
        e.insert(format!("m/extra/{k}"), v.clone());
    }
    Ok(e)
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
fn coverage_entries(cov: &Value, e: &mut Entries) {
    if cov.is_null() {
        return;
    }
    let split = cov.get("regions").and_then(Value::as_array).filter(|rs| {
        let ids: std::collections::BTreeSet<&str> =
            rs.iter().filter_map(|r| r.get("id").and_then(Value::as_str)).collect();
        ids.len() == rs.len()
    });
    let (Some(rs), Some(o)) = (split, cov.as_object()) else {
        e.insert("m/coverage".into(), cov.clone());
        return;
    };
    for (i, r) in rs.iter().enumerate() {
        let id = r.get("id").and_then(Value::as_str).expect("checked above");
        e.insert(format!("r/{id}"), serde_json::json!({"i": i, "region": r}));
    }
    let mut rest = o.clone();
    rest.remove("regions");
    e.insert("m/coverage".into(), Value::Object(rest));
    e.insert("m/regions".into(), Value::from(rs.len()));
}

/// The catalogue entries make, numbered `n` and made at `created` (§4.1): `entries`' inverse.
pub fn catalogue(e: &Entries, n: u64, created: &str) -> Result<Catalogue> {
    let mut c = Catalogue { n, created: created.to_string(), ..Default::default() };
    let mut layer_parts: Vec<(&str, &str, &str, &Value)> = Vec::new();
    let mut regions: Vec<(u64, Value)> = Vec::new();
    let (mut cov, mut n_regions) = (None, None);
    let s = |v: &Value, k: &str| v.as_str().map(str::to_string).with_context(|| format!("{k}: not a name"));
    for (k, v) in e {
        let (kind, rest) = k.split_once('/').with_context(|| format!("entry {k:?} has no kind"))?;
        match kind {
            "f" => {
                c.files.insert(rest.into(), serde_json::from_value::<FileRef>(v.clone()).with_context(|| k.clone())?);
            }
            "l" => match rest.split_once('/') {
                None => {
                    c.layers
                        .insert(rest.into(), serde_json::from_value::<Layer>(v.clone()).with_context(|| k.clone())?);
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
                let i = v.get("i").and_then(Value::as_u64).with_context(|| format!("entry {k:?}: no place"))?;
                regions.push((i, v.get("region").cloned().unwrap_or_default()));
            }
            "m" => match rest {
                "fmt" => c.fmt = serde_json::from_value(v.clone()).context("m/fmt")?,
                "app" => c.app = s(v, k)?,
                "basemap" => c.basemap = serde_json::from_value(v.clone()).context("m/basemap")?,
                "meta" => c.meta = v.clone(),
                "credits" => c.credits = v.clone(),
                "dem_labels" => c.dem_labels = serde_json::from_value(v.clone()).context("m/dem_labels")?,
                "units" => c.units = serde_json::from_value(v.clone()).context("m/units")?,
                "coverage" => cov = Some(v.clone()),
                "regions" => n_regions = Some(v.as_u64().context("m/regions")? as usize),
                _ => match rest.strip_prefix("extra/") {
                    Some(x) => {
                        c.extra.insert(x.into(), v.clone());
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
                let z = v
                    .as_u64()
                    .and_then(|z| u8::try_from(z).ok())
                    .with_context(|| format!("{name}'s zoom of {tile}"))?;
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
}

/// An `f/` entry's content name.
fn content_of(v: &Value) -> Option<&str> {
    v.get("file").and_then(Value::as_str)
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
        let mut set = Entries::new();
        let mut d = self.digest;
        let mut freed = Vec::new();
        for (k, v) in next {
            let old = self.entries.get(k);
            if old == Some(v) {
                continue;
            }
            if let Some(o) = old {
                d = d.wrapping_sub(entry_hash(k, o));
                if let Some(f) = content_of(o).filter(|f| k.starts_with("f/") && content_of(v) != Some(f)) {
                    freed.push(f.to_string());
                }
            }
            d = d.wrapping_add(entry_hash(k, v));
            set.insert(k.clone(), v.clone());
        }
        let mut del = Vec::new();
        for (k, o) in self.entries.iter().filter(|(k, _)| !next.contains_key(*k)) {
            d = d.wrapping_sub(entry_hash(k, o));
            if let Some(f) = content_of(o).filter(|_| k.starts_with("f/")) {
                freed.push(f.to_string());
            }
            del.push(k.clone());
        }
        freed.sort();
        Batch { seq, at: at.into(), app: app.into(), why: why.into(), set, del, freed, digest: hex(d) }
    }

    /// The map after `b`, applied to a copy: this one is left as it was when `b` doesn't apply (a
    /// `seq` not past this one's, an entry deleted that isn't there, or the digest after it not the
    /// batch's).
    pub fn apply(&self, b: &Batch) -> Result<Map> {
        let mut m = self.clone();
        m.apply_in(b)?;
        Ok(m)
    }

    /// `batches` applied in turn to one copy, swapped in once by the caller.
    pub fn apply_all(&self, batches: &[Batch]) -> Result<Map> {
        let mut m = self.clone();
        for b in batches {
            m.apply_in(b)?;
        }
        Ok(m)
    }

    fn apply_in(&mut self, b: &Batch) -> Result<()> {
        ensure!(b.seq > self.seq, "batch {} isn't past the map's {}", b.seq, self.seq);
        let want = unhex(&b.digest)?;
        let mut d = self.digest;
        for k in &b.del {
            let o =
                self.entries.remove(k).with_context(|| format!("batch {} deletes {k}, which isn't there", b.seq))?;
            d = d.wrapping_sub(entry_hash(k, &o));
        }
        for (k, v) in &b.set {
            if let Some(o) = self.entries.insert(k.clone(), v.clone()) {
                d = d.wrapping_sub(entry_hash(k, &o));
            }
            d = d.wrapping_add(entry_hash(k, v));
        }
        ensure!(d == want, "batch {}: the map after it has digest {}, the batch says {}", b.seq, hex(d), b.digest);
        self.digest = d;
        self.seq = b.seq;
        self.at = b.at.clone();
        Ok(())
    }

    /// Whether the digest kept equals the one of its entries from scratch.
    pub fn digest_holds(&self) -> bool {
        digest_of(&self.entries) == self.digest
    }
}

/// A client's cursor (§5.4): the last `seq` it applied and when it was heard (seconds since 1970).
/// Saved with the log's snapshot (Decisions, 6).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    pub seq: u64,
    pub heard: u64,
}

/// The whole map as stored: the lead's snapshot and the NAS's (§4.4, §4.5).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub seq: u64,
    pub at: String,
    pub digest: String,
    pub entries: Entries,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cursors: BTreeMap<String, Cursor>,
}

impl Snapshot {
    pub fn of(m: &Map, cursors: BTreeMap<String, Cursor>) -> Snapshot {
        Snapshot { seq: m.seq, at: m.at.clone(), digest: hex(m.digest), entries: m.entries.clone(), cursors }
    }

    /// Compact JSON, zstd (level 3: written hourly, read at a start) with its content checksum.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(self)?;
        let mut enc = zstd::stream::Encoder::new(Vec::with_capacity(json.len() / 4 + 64), 3)?;
        enc.include_checksum(true)?;
        enc.write_all(&json)?;
        Ok(enc.finish()?)
    }

    /// The map and cursors it holds: an error when it's damaged or its entries aren't its digest.
    pub fn decode(bytes: &[u8]) -> Result<(Map, BTreeMap<String, Cursor>)> {
        let json = zstd::stream::decode_all(bytes).context("the map's snapshot doesn't decompress")?;
        let s: Snapshot = serde_json::from_slice(&json).context("the map's snapshot doesn't parse")?;
        let m = Map::new(s.seq, s.at, s.entries);
        ensure!(hex(m.digest) == s.digest, "the map's snapshot {} isn't its digest", s.seq);
        Ok((m, s.cursors))
    }
}

/// `map/<seq>.json.zst`'s name.
pub fn nas_name(seq: u64) -> String {
    format!("{seq}{SUFFIX}")
}

/// Writes the map whole to the NAS (`map/<seq>.json.zst`, a temporary name renamed into place: §4.4),
/// then removes all but the newest `NAS_KEPT` there. Once an hour at most (the lead's caller).
pub fn write_nas_snapshot(root: &NasRoot, m: &Map) -> Result<()> {
    let dir = root.join(NAS_DIR);
    let b = Snapshot::of(m, BTreeMap::new()).encode()?;
    dir.join(nas_name(m.seq)).write_whole_made(&b).with_context(|| format!("write {NAS_DIR}/{}", nas_name(m.seq)))?;
    let mut seqs: Vec<u64> = dir
        .list()
        .with_context(|| format!("list {NAS_DIR}"))?
        .iter()
        .filter_map(|n| n.strip_suffix(SUFFIX)?.parse().ok())
        .collect();
    seqs.sort_unstable_by(|a, b| b.cmp(a));
    for s in seqs.into_iter().skip(NAS_KEPT) {
        if let Err(e) = dir.join(nas_name(s)).remove_file() {
            eprintln!("maplog: can't remove {NAS_DIR}/{}: {e}", nas_name(s));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
