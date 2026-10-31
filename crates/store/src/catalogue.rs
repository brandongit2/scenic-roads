//! Catalogues (docs/formats.md, Catalogue): the only index of what's on the NAS. Each publish writes
//! the next `catalog/<n>.json.zst` (zstd with its content checksum on), naming every current file
//! by content name with its size and format version. Readers take the highest `<n>` that decodes
//! and parses, falling back past a damaged or half-copied one. A Mac keeps copies of the last few
//! it read in its own `catalog/`, for starting offline.

use crate::iopool::{IoError, IoPool};
use crate::naming::{parse_content_name, rename_no_replace, tmp_path};
use crate::nasfs::{Loc, NasPath, Place};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The stored and wire name of a catalogue: the NAS folder (`catalog/<n>.json.zst`), the build step
/// and the history event that publish one, and the JSON key naming it. It keeps its old spelling
/// (task #547: the NAS holds years of data under it, and the pool runs mixed app versions): only the
/// Rust names around it say "catalogue". JSON keys and serde names elsewhere keep it with a comment.
pub const NAME: &str = "catalog";
/// The same for a catalogue held for review: its folder (`catalog-held/`) and its step.
pub const HELD_NAME: &str = "catalog-held";

/// The catalogue format version written. (2, task #449: `markdata` may name one bundle of every
/// tile, docs/formats.md "Bundles", which an app reading only 1 can't read: it keeps its last
/// catalogue of format 1 until it updates.)
pub const FMT: u32 = 2;
/// The oldest version read (docs/building.md §2).
pub const MIN_FMT: u32 = 1;

const LEVEL: i32 = 9;
const SUFFIX: &str = ".json.zst";
const DELTA_SUFFIX: &str = ".delta.json.zst";

pub mod delta;

/// One catalogue (docs/formats.md). Maps are `BTreeMap`s so the JSON comes out the same for the same
/// catalogue; fields this version doesn't know are kept in `extra`, so nothing is lost when a
/// catalogue is read and written again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalogue {
    /// The catalogue format version.
    pub fmt: u32,
    /// The publish number, also the file's name.
    pub n: u64,
    /// RFC 3339 time of the publish.
    #[serde(default)]
    pub created: String,
    /// The published app that made it (its folder's name), or "development".
    #[serde(default)]
    pub app: String,
    /// Every file the catalogue references, by logical name. GC keeps them, and the files of the last
    /// 14 days' catalogues (docs/storage.md).
    #[serde(default)]
    pub files: BTreeMap<String, FileRef>,
    /// The built z6 tiles (tile keys as "z/x/y"), sorted. (Stored and served as `units`: catalogues on the NAS and
    /// the web client read it so; kept.)
    #[serde(default)]
    pub units: Vec<String>,
    #[serde(default)]
    pub layers: BTreeMap<String, Layer>,
    /// Logical names of the basemap's PMTiles files.
    #[serde(default)]
    pub basemap: Vec<String>,
    /// Base packs, road values and hi data by tile ("z/x/y" → logical name).
    #[serde(default)]
    pub base: BTreeMap<String, String>,
    #[serde(default)]
    pub roads: BTreeMap<String, String>,
    #[serde(default)]
    pub hidata: BTreeMap<String, String>,
    /// Landmark points per z6 tile ("6/x/y" → logical name; docs/phase5.md).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub markdata: BTreeMap<String, String>,
    /// Area overlays' details and the parks' records per z3 tile ("3/x/y" → logical name).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ovdata: BTreeMap<String, String>,
    /// Small worldwide files by what they are (e.g. "pois.json" → logical name).
    #[serde(default)]
    pub global: BTreeMap<String, String>,
    /// The app's meta (bounds, elevation histogram, DEM counts …).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub meta: Value,
    /// The credits of the sources its data comes from: `[{"what", "source", "terms", "areas"?}]`
    /// (pipeline::rules::Credit). Empty in catalogues made before they were recorded. Kept as JSON,
    /// as `coverage` is, so a part the app can't read never makes the catalogue unreadable.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub credits: Value,
    /// The elevation datasets' labels by source byte (`src.u8`), from their declarations
    /// (pipeline::inputs::credits::dem_labels): how the map names a road's elevation sources. None
    /// in catalogues made before they were recorded.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dem_labels: BTreeMap<u8, String>,
    /// The coverage it was built for: `{"regions": [{"id", "name", "outline", "shapes"}]}`, each
    /// outline entry's polygons simplified for drawing (pipeline::coverage::DrawnRegion). No
    /// regions in catalogues made before they were recorded.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub coverage: Value,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A referenced file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    /// The content name, relative to the NAS root.
    pub file: String,
    pub size: u64,
    /// The file's own format version.
    #[serde(default)]
    pub fmt: u32,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One of our tile layers: its root, lo and hi packs by root tile.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    #[serde(default)]
    pub encoding: String,
    #[serde(default)]
    pub minzoom: u8,
    #[serde(default)]
    pub maxzoom: u8,
    /// The root pack (z0–2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Lo packs (z3–8) by z3 tile.
    #[serde(default)]
    pub lo: BTreeMap<String, String>,
    /// Hi packs (z9–14) by z6 tile.
    #[serde(default)]
    pub hi: BTreeMap<String, String>,
    /// Deep packs (z13 and finer) by z10 tile: the terrain's where a dataset is finer than the
    /// worldwide ones (pipeline::terrain_deep).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deep: BTreeMap<String, String>,
    /// The native zoom of each deep pack's z10 tile, past the base's (pipeline::terrain_deep::
    /// native_zoom; elsewhere the base's finest, z12 to 67°): the map's zoom by area.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub native_zoom: BTreeMap<String, u8>,
    /// A polar layer's block packs (z4–9) by z4 tile of its pole's grid ("4/x/y": pipeline::polar,
    /// docs/polar.md); its `root` then holds z0–3. A polar layer is named `polar-<kind>-<pole>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub polar: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Catalogue {
    /// An empty catalogue number `n` in the current format.
    pub fn new(n: u64) -> Self {
        Self { fmt: FMT, n, ..Default::default() }
    }

    pub fn file(&self, logical: &str) -> Option<&FileRef> {
        self.files.get(logical)
    }

    /// The content name of a logical file.
    pub fn content(&self, logical: &str) -> Option<&str> {
        self.files.get(logical).map(|f| f.file.as_str())
    }

    /// Every logical name the catalogue's sections refer to, with where it's referred from.
    pub fn references(&self) -> Vec<(String, &str)> {
        let mut out = Vec::new();
        for (name, l) in &self.layers {
            if let Some(r) = &l.root {
                out.push((format!("layers.{name}.root"), r.as_str()));
            }
            for (k, v) in l.lo.iter().chain(&l.hi).chain(&l.deep).chain(&l.polar) {
                out.push((format!("layers.{name}[{k}]"), v.as_str()));
            }
        }
        for b in &self.basemap {
            out.push(("basemap".into(), b.as_str()));
        }
        for (what, m) in [
            ("base", &self.base),
            ("roads", &self.roads),
            ("hidata", &self.hidata),
            ("markdata", &self.markdata),
            ("ovdata", &self.ovdata),
            ("global", &self.global),
        ] {
            for (k, v) in m {
                out.push((format!("{what}[{k}]"), v.as_str()));
            }
        }
        out
    }

    /// Checks that every reference resolves through `files`, and that every file's content name
    /// is well formed and belongs to its logical name. Publishers check before writing.
    pub fn validate(&self) -> Result<()> {
        ensure!(self.fmt == FMT, "catalogue format {} (this app writes {FMT})", self.fmt);
        for (logical, f) in &self.files {
            let c = parse_content_name(&f.file)
                .with_context(|| format!("files[{logical}]: {:?} isn't a content name", f.file))?;
            ensure!(c.logical == logical, "files[{logical}] names a file of {}", c.logical);
        }
        for (from, logical) in self.references() {
            ensure!(self.files.contains_key(logical), "{from} refers to {logical}, which isn't in files");
        }
        Ok(())
    }

    /// The catalogue as stored: compact JSON, zstd level 9 with its content checksum.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(self)?;
        let mut enc = zstd::stream::Encoder::new(Vec::with_capacity(json.len() / 4 + 64), LEVEL)?;
        enc.include_checksum(true)?;
        enc.write_all(&json)?;
        Ok(enc.finish()?)
    }

    /// Decodes a stored catalogue: an error when the zstd frame is damaged or cut short (the checksum
    /// catches what the format doesn't), the JSON doesn't parse, or the format isn't one this app
    /// reads.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let json = zstd::stream::decode_all(bytes).context("catalogue doesn't decompress")?;
        let c: Catalogue = serde_json::from_slice(&json).context("catalogue doesn't parse")?;
        ensure!(
            (MIN_FMT..=FMT).contains(&c.fmt),
            "catalogue format {} isn't supported (this app reads {MIN_FMT}–{FMT})",
            c.fmt
        );
        Ok(c)
    }
}

/// `<n>.json.zst` → n.
fn number(name: &str) -> Option<u64> {
    digits(name.strip_suffix(SUFFIX)?)
}

/// `<n>.delta.json.zst` → n.
fn delta_number(name: &str) -> Option<u64> {
    digits(name.strip_suffix(DELTA_SUFFIX)?)
}

fn digits(d: &str) -> Option<u64> {
    if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    d.parse().ok()
}

/// The file name of catalogue `n`, whole.
pub fn file_name(n: u64) -> String {
    format!("{n}{SUFFIX}")
}

/// The file name of catalogue `n` written as what changed over the one before it (`Delta`).
pub fn delta_file_name(n: u64) -> String {
    format!("{n}{DELTA_SUFFIX}")
}

/// Publishes `cat` into `dir` as `<n>.json.zst` after checking it (`Catalogue::validate`).
pub fn write(dir: &Path, cat: &Catalogue) -> Result<PathBuf> {
    cat.validate().with_context(|| format!("catalogue {}", cat.n))?;
    ensure!(!dir.join(delta_file_name(cat.n)).exists(), "catalogue {} exists already, as a delta", cat.n);
    write_copy(dir, cat)
}

/// Writes `cat` into `dir` as `<n>.json.zst`, unchecked: for keeping a copy of a catalogue read
/// elsewhere (a Mac's own `catalog/`). The bytes go to `<n>.json.zst.tmp`, are decoded back and
/// compared, then renamed into place. A catalogue number is written once: if it exists with the
/// same content this does nothing, and with other content it's an error.
pub fn write_copy(dir: &Path, cat: &Catalogue) -> Result<PathBuf> {
    let path = dir.join(file_name(cat.n));
    if let Some(done) = same_as_existing(&path, cat)? {
        return Ok(done);
    }
    let bytes = cat.encode()?;
    let check = |b: &[u8]| -> Result<()> {
        let back = Catalogue::decode(b)?;
        ensure!(&back == cat, "catalogue read back differs from what was written");
        Ok(())
    };
    match put_once(dir, &path, &bytes, &check)? {
        true => Ok(path),
        false => same_as_existing(&path, cat)?
            .with_context(|| format!("{} appeared meanwhile with other content", path.display())),
    }
}

/// Writes `bytes` to `path` (in `dir`) through a temporary file read back by `check`, never over a
/// file that's there: false when one appeared meanwhile (nothing written).
fn put_once(dir: &Path, path: &Path, bytes: &[u8], check: &dyn Fn(&[u8]) -> Result<()>) -> Result<bool> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let tmp = tmp_path(path);
    let written = (|| -> Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        check(&fs::read(&tmp)?)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e.context(format!("write {}", tmp.display())));
    }
    match rename_no_replace(&tmp, path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp);
            Ok(false)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e).with_context(|| format!("rename into {}", path.display()))
        }
    }
}

/// `Some(path)` when the catalogue file exists and holds `cat`; None when it doesn't exist; an error
/// when it holds something else.
fn same_as_existing(path: &Path, cat: &Catalogue) -> Result<Option<PathBuf>> {
    match fs::read(path) {
        Ok(b) => match Catalogue::decode(&b) {
            Ok(old) if &old == cat => Ok(Some(path.to_owned())),
            Ok(_) => bail!("{} exists with other content; a catalogue number is never reused", path.display()),
            Err(e) => bail!("{} exists but is damaged ({e:#}); it must be removed by hand", path.display()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Reads one whole catalogue file (`read_n` reads a catalogue by number, whichever way it was written).
pub fn read<'a>(path: impl Into<Loc<'a>>) -> Result<Catalogue> {
    let path = path.into();
    let b = path.read().with_context(|| format!("read {}", path.display()))?;
    Catalogue::decode(&b).with_context(|| path.display())
}

/// Catalogue `n` in `dir`, whole: its own file, or what changed over the one before it applied to
/// that one (`Delta`), back to a whole one.
pub fn read_n<'a>(dir: impl Into<Loc<'a>>, n: u64) -> Result<Catalogue> {
    read_n_from(dir, n, None)
}

/// `read_n` from a catalogue read before (`have`): when it's on catalogue `n`'s chain, only the deltas
/// after it are read and applied, not the chain back to a whole catalogue.
/// Each delta checks the catalogue it changes by its digest, so a `have` that isn't the one now
/// numbered so (a number taken again) fails there, and `n` is read without it. `have` taken for
/// `n` itself is checked against `n`'s delta's digest first (a whole `n` is read).
pub fn read_n_from<'a>(dir: impl Into<Loc<'a>>, n: u64, have: Option<&Catalogue>) -> Result<Catalogue> {
    let dir = dir.into();
    let names = Names::of_dir(dir)?;
    let mut read = |f: &str| local_read(dir.join(f).loc());
    if let Some(h) = have.filter(|h| h.n == n && !names.whole.contains(&n) && names.delta.contains(&n)) {
        if let (Ok(Some(b)), Ok(dg)) = (read(&delta_file_name(n)), digest(h)) {
            if Delta::decode(&b).is_ok_and(|d| d.n == n && d.digest == dg) {
                return Ok(h.clone());
            }
        }
    }
    let shown = dir.display();
    let have = have.filter(|h| h.n < n);
    if have.is_some() {
        if let Some(c) = materialize(Path::new(&shown), &names, n, have, &mut read)? {
            return Ok(c);
        }
    }
    materialize(Path::new(&shown), &names, n, None, &mut read)?
        .with_context(|| format!("catalogue {n} in {} can't be read whole", dir.display()))
}

/// The file catalogue `n` is in (whole, or as a delta), for its time: None when neither is there.
pub fn path_n<'a>(dir: impl Into<Loc<'a>>, n: u64) -> Option<Place> {
    let dir = dir.into();
    [file_name(n), delta_file_name(n)].into_iter().map(|f| dir.join(&f)).find(|p| p.loc().exists())
}

/// The catalogue numbers in `dir`, whole or deltas, highest first (none when `dir` doesn't exist).
pub fn list<'a>(dir: impl Into<Loc<'a>>) -> Result<Vec<u64>> {
    Ok(Names::of_dir(dir.into())?.numbers())
}

/// The catalogues in a folder by kind.
#[derive(Default)]
struct Names {
    whole: BTreeSet<u64>,
    delta: BTreeSet<u64>,
}

impl Names {
    fn add(&mut self, name: &str) {
        if let Some(n) = number(name) {
            self.whole.insert(n);
        } else if let Some(n) = delta_number(name) {
            self.delta.insert(n);
        }
    }

    fn of_dir(dir: Loc<'_>) -> Result<Self> {
        let mut names = Names::default();
        let listed = match dir.list_names() {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(names),
            Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
        };
        for name in &listed {
            names.add(name);
        }
        Ok(names)
    }

    /// Highest first.
    fn numbers(&self) -> Vec<u64> {
        let mut ns: Vec<u64> = self.whole.union(&self.delta).copied().collect();
        ns.reverse();
        ns
    }
}

fn local_read(p: Loc<'_>) -> Result<Option<Vec<u8>>> {
    match p.read() {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            eprintln!("catalogue: skipping {}: {e}", p.display());
            Ok(None)
        }
    }
}

/// The highest catalogue in `dir` that reads whole; damaged ones are logged and skipped.
pub fn latest<'a>(dir: impl Into<Loc<'a>>) -> Result<Option<Catalogue>> {
    let dir = dir.into();
    let names = Names::of_dir(dir)?;
    pick(Path::new(&dir.display()), &names, None, &mut |n| local_read(dir.join(n).loc()))
}

/// `latest` for a folder on the NAS read directly (crate::nasfs: counted, its reads retried), not
/// through the I/O pool: a build's or the CLI's.
pub fn latest_on(dir: &NasPath) -> Result<Option<Catalogue>> {
    let mut names = Names::default();
    let entries = match dir.read_dir() {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("list {dir:?}")),
    };
    for e in &entries {
        names.add(&e.name);
    }
    pick(Path::new(&format!("{dir:?}")), &names, None, &mut |n| match dir.join(n).read() {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            eprintln!("catalogue: skipping {dir:?}/{n}: {e}");
            Ok(None)
        }
    })
}

/// `latest` for a directory on the NAS: every listing and read goes through the I/O pool. The
/// NAS going offline midway is an error, never a reason to fall back to an older catalogue.
pub fn latest_nas(pool: &IoPool, dir: &NasPath) -> Result<Option<Catalogue>> {
    latest_nas_from(pool, dir, None)
}

/// `latest_nas` from a catalogue this Mac read before (`have`): the newest is `have` itself when its
/// number is the highest there (nothing read but the listing), and the deltas after it are read
/// and applied to it, not the chain back to a whole catalogue.
pub fn latest_nas_from(pool: &IoPool, dir: &NasPath, have: Option<&Catalogue>) -> Result<Option<Catalogue>> {
    let mut names = Names::default();
    match pool.list(dir) {
        Ok(items) => {
            for i in items.iter().filter(|i| !i.is_dir) {
                names.add(&i.name);
            }
        }
        Err(IoError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("list {dir:?}")),
    }
    if let Some(h) = have.filter(|h| names.numbers().first() == Some(&h.n)) {
        return Ok(Some(h.clone()));
    }
    pick(Path::new(&format!("{dir:?}")), &names, have, &mut |n| match pool.read_all(&dir.join(n)) {
        Ok(b) => Ok(Some(b)),
        Err(IoError::Io(e)) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(IoError::Io(e)) => {
            eprintln!("catalogue: skipping {dir:?}/{n}: {e}");
            Ok(None)
        }
        Err(e) => Err(e).with_context(|| format!("read {dir:?}/{n}")),
    })
}

/// The first catalogue of `names` (highest first) that reads whole.
fn pick(
    dir: &Path,
    names: &Names,
    have: Option<&Catalogue>,
    read: &mut dyn FnMut(&str) -> Result<Option<Vec<u8>>>,
) -> Result<Option<Catalogue>> {
    for n in names.numbers() {
        if let Some(c) = materialize(dir, names, n, have, read)? {
            return Ok(Some(c));
        }
        // (`have` not the one its number now names: `n` read without it, not passed over.)
        if have.is_some_and(|h| h.n < n) {
            if let Some(c) = materialize(dir, names, n, None, read)? {
                return Ok(Some(c));
            }
        }
    }
    Ok(None)
}

/// Catalogue `n` whole: its own file, or its delta applied to the one before it (`have` when that's
/// the one, or this Mac's copy of it (`keep_read_in`), else read the same way). The deltas are
/// applied to one JSON value (task #895: not the catalogue turned to JSON, hashed and parsed again
/// at each), checked by the digests they carry: the first's `of_digest` against the catalogue it
/// starts from, each one's against the `digest` of the one before, the last's `digest` against
/// what they make. None (logged) when a file is damaged,
/// missing or doesn't carry its own number, or a digest differs (`first_wrong` names which delta).
fn materialize(
    dir: &Path,
    names: &Names,
    n: u64,
    have: Option<&Catalogue>,
    read: &mut dyn FnMut(&str) -> Result<Option<Vec<u8>>>,
) -> Result<Option<Catalogue>> {
    let mut chain: Vec<Delta> = Vec::new();
    let mut at = n;
    // (The catalogue the chain starts from, with its digest when that's known already.)
    let (base, base_digest) = loop {
        if let Some(h) = have.filter(|h| h.n == at) {
            break (h.clone(), None);
        }
        if chain.len() > MAX_CHAIN {
            eprintln!("catalogue: skipping {n}: its deltas go back more than {MAX_CHAIN}");
            return Ok(None);
        }
        if names.whole.contains(&at) {
            let p = dir.join(file_name(at));
            let Some(bytes) = read(&file_name(at))? else { return Ok(None) };
            match Catalogue::decode(&bytes) {
                Ok(c) if c.n == at => break (c, None),
                Ok(c) => eprintln!("catalogue: skipping {}: it says it is number {}", p.display(), c.n),
                Err(e) => eprintln!("catalogue: skipping {}: {e:#}", p.display()),
            }
            return Ok(None);
        }
        if !names.delta.contains(&at) {
            eprintln!("catalogue: skipping {n}: catalogue {at}, which it changes, isn't there");
            return Ok(None);
        }
        let p = dir.join(delta_file_name(at));
        let Some(bytes) = read(&delta_file_name(at))? else { return Ok(None) };
        match Delta::decode(&bytes) {
            Ok(d) if d.n == at && d.of < at => {
                if chain.is_empty() {
                    if let Some(c) = kept(d.n, &d.digest) {
                        return Ok(Some(c));
                    }
                }
                at = d.of;
                let from = kept(d.of, &d.of_digest).map(|c| (c, d.of_digest.clone()));
                chain.push(d);
                if let Some((c, dg)) = from {
                    break (c, Some(dg));
                }
            }
            Ok(d) => {
                eprintln!("catalogue: skipping {}: it says it is number {} over {}", p.display(), d.n, d.of);
                return Ok(None);
            }
            Err(e) => {
                eprintln!("catalogue: skipping {}: {e:#}", p.display());
                return Ok(None);
            }
        }
    };
    let Some(last) = chain.first().map(|d| d.digest.clone()) else { return Ok(Some(base)) };
    if chain.len() >= MAX_CHAIN / 2 {
        eprintln!(
            "catalogue: {n} is read through {} deltas (at most {MAX_CHAIN}): its writer should have written a whole one",
            chain.len()
        );
    }
    chain.reverse();
    let first = &chain[0];
    let base_digest = match base_digest {
        Some(d) => d,
        None => digest(&base)?,
    };
    if base_digest != first.of_digest {
        eprintln!(
            "catalogue: skipping {n}: delta {} over {}: catalogue {} isn't the one it changes (digests differ)",
            first.n, first.of, first.of
        );
        return Ok(None);
    }
    // (Each delta changes the one the one before makes, by their digests: no catalogue's hashed.)
    if let Some(w) = chain.windows(2).find(|w| w[1].of_digest != w[0].digest) {
        eprintln!(
            "catalogue: skipping {n}: delta {} over {}: catalogue {} isn't the one it changes (digests differ)",
            w[1].n, w[1].of, w[1].of
        );
        return Ok(None);
    }
    let made = (|| -> Result<Catalogue> {
        let mut v = serde_json::to_value(&base)?;
        for d in &chain {
            delta::apply_in(&mut v, &d.patch).with_context(|| format!("delta {} over {}", d.n, d.of))?;
        }
        serde_json::from_value(v).context("the patched catalogue doesn't parse")
    })();
    match made {
        Ok(c) if c.n == n && digest(&c)? == last => {
            keep(&c, &last);
            Ok(Some(c))
        }
        Ok(_) | Err(_) => {
            let why = match made {
                Err(e) => format!("{e:#}"),
                Ok(_) => first_wrong(&base, &chain),
            };
            eprintln!("catalogue: skipping {n}: {why}");
            Ok(None)
        }
    }
}

/// Which of `chain` (applied in order to `base`, which its first changes) first makes a catalogue
/// other than the one it names by its digest: found by bisection, each try applying the chain
/// that far to `base` again and hashing what it makes once.
fn first_wrong(base: &Catalogue, chain: &[Delta]) -> String {
    let made = |k: usize| -> Result<Catalogue> {
        let mut v = serde_json::to_value(base)?;
        for d in &chain[..k] {
            delta::apply_in(&mut v, &d.patch)?;
        }
        Ok(serde_json::from_value(v)?)
    };
    let right =
        |k: usize| made(k).is_ok_and(|c| c.n == chain[k - 1].n && digest(&c).is_ok_and(|g| g == chain[k - 1].digest));
    let (mut lo, mut hi) = (0, chain.len());
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if right(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let d = &chain[hi - 1];
    format!("delta {} over {}: applied, it isn't catalogue {} (digests differ)", d.n, d.of, d.n)
}

/// Where this process keeps the last catalogue it made from deltas (`keep_read_in`), if anywhere.
static KEPT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

#[cfg(not(test))]
fn kept_in() -> Option<PathBuf> {
    KEPT.get().cloned()
}

// (A test keeps its own, on its own thread: the tests' catalogues are alike, and one test's kept
// catalogue would be another's chain's start.)
#[cfg(test)]
thread_local!(static TEST_KEPT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) });
#[cfg(test)]
fn kept_in() -> Option<PathBuf> {
    TEST_KEPT.with(|k| k.borrow().clone())
}

/// From now on, each catalogue this process makes from deltas is kept in `dir` (this Mac's, a cache:
/// `<n>-<digest>.json.zst`, the two of highest numbers), and a later read of a chain through it, by any
/// process here, starts from it rather than from the whole catalogue the chain goes back to (task
/// #895: a restart applies only the deltas since). The apps, the agent and the build set it; tests
/// don't. Set once: later calls change nothing.
pub fn keep_read_in(dir: PathBuf) {
    #[cfg(test)]
    TEST_KEPT.with(|k| *k.borrow_mut() = Some(dir.clone()));
    let _ = KEPT.set(dir);
}

/// This Mac's cache folder for `keep_read_in`: `~/Library/Caches/scenic/catalogue` (None without a home).
pub fn kept_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches/scenic/catalogue"))
}

fn kept_name(n: u64, digest: &str) -> String {
    format!("{n}-{digest}{SUFFIX}")
}

/// Catalogue `n` of digest `digest`, as kept here (`keep_read_in`): None when it isn't, or doesn't
/// read as that one.
fn kept(n: u64, dg: &str) -> Option<Catalogue> {
    let b = fs::read(kept_in()?.join(kept_name(n, dg))).ok()?;
    Catalogue::decode(&b).ok().filter(|c| c.n == n && digest(c).is_ok_and(|g| g == dg))
}

/// Keeps `cat` (of digest `dg`) here (`keep_read_in`), with one other: a failure is logged, the
/// read it came from goes on.
fn keep(cat: &Catalogue, dg: &str) {
    let Some(dir) = kept_in() else { return };
    let name = kept_name(cat.n, dg);
    let made = (|| -> Result<()> {
        fs::create_dir_all(&dir)?;
        // (Written even when the name's there: a catalogue is made from deltas only when the kept
        // one of its name didn't read as it, `kept`, so that file is damaged, a torn write.)
        let path = dir.join(&name);
        let json = serde_json::to_vec(cat)?;
        // (Fast to write: it's this Mac's, kept for its next start, not stored on the NAS.)
        let mut enc = zstd::stream::Encoder::new(Vec::with_capacity(json.len() / 4 + 64), 1)?;
        enc.include_checksum(true)?;
        enc.write_all(&json)?;
        // (A temporary name of its own, this process's and this write's: two threads of one server
        // keeping the same catalogue never write one file; a reader checks the digest anyway.)
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut tmp = tmp_path(&path).into_os_string();
        tmp.push(format!(".{}", WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let tmp = PathBuf::from(tmp);
        fs::write(&tmp, enc.finish()?)?;
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        // (The two highest numbers stay: a held catalogue's chain and the served one's alternate,
        // and a test's small catalogue never pushes out the map's.)
        // (A temporary file an hour old was left by a process that stopped mid-write: removed, so
        // the folder holds two catalogues and the writes under way, nothing more.)
        let hour_old = |e: &fs::DirEntry| {
            e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|a| a.as_secs() > 3600))
        };
        let mut all: Vec<(u64, PathBuf)> = Vec::new();
        for e in fs::read_dir(&dir)?.flatten() {
            let name = e.file_name();
            let Some(f) = name.to_str() else { continue };
            match f.strip_suffix(SUFFIX).and_then(|f| f.split_once('-')?.0.parse().ok()) {
                Some(k) => all.push((k, e.path())),
                None if f.ends_with(".tmp") || f.contains(".tmp.") => {
                    if hour_old(&e) {
                        let _ = fs::remove_file(e.path());
                    }
                }
                None => {}
            }
        }
        all.sort();
        all.reverse();
        for (_, p) in all.into_iter().skip(2) {
            let _ = fs::remove_file(p);
        }
        Ok(())
    })();
    if let Err(e) = made {
        eprintln!("catalogue: can't keep catalogue {} in {}: {e:#}", cat.n, dir.display());
    }
}

/// The longest run of deltas read back to a whole catalogue (a writer writes a whole one long
/// before: `Delta::due_whole`).
const MAX_CHAIN: usize = 1000;

/// A catalogue written as what changed over the one before it (docs/formats.md, Delta catalogues):
/// `catalog/<n>.delta.json.zst`, zstd with its content checksum. This app's readers apply it to
/// catalogue `of` (whole, or itself a delta) and get catalogue `n` exactly as the whole one would be:
/// `of_digest` and `digest` (`digest`) check both ends. An older app's readers don't list it (its
/// name isn't `<n>.json.zst`): they keep to the last whole catalogue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub fmt: u32,
    pub n: u64,
    /// The catalogue it changes.
    pub of: u64,
    pub of_digest: String,
    /// Catalogue `n`'s digest, whole.
    pub digest: String,
    /// `delta::diff` of the two as JSON.
    pub patch: Value,
}

impl Delta {
    /// The delta from `base` to `cat`.
    pub fn between(base: &Catalogue, cat: &Catalogue) -> Result<Self> {
        let patch = delta::diff(&serde_json::to_value(base)?, &serde_json::to_value(cat)?);
        Ok(Delta { fmt: FMT, n: cat.n, of: base.n, of_digest: digest(base)?, digest: digest(cat)?, patch })
    }

    /// Catalogue `n`: this applied to `base` (catalogue `of`), checked by the digests.
    pub fn apply(&self, base: &Catalogue) -> Result<Catalogue> {
        ensure!(base.n == self.of, "it changes catalogue {}, not {}", self.of, base.n);
        ensure!(digest(base)? == self.of_digest, "catalogue {} isn't the one it changes (digests differ)", base.n);
        let mut v = serde_json::to_value(base)?;
        delta::apply_in(&mut v, &self.patch)?;
        let c: Catalogue = serde_json::from_value(v).context("the patched catalogue doesn't parse")?;
        ensure!(c.n == self.n && digest(&c)? == self.digest, "applied, it isn't catalogue {} (digests differ)", self.n);
        Ok(c)
    }

    /// As stored: compact JSON, zstd level 9 with its content checksum.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(self)?;
        let mut enc = zstd::stream::Encoder::new(Vec::with_capacity(json.len() / 4 + 64), LEVEL)?;
        enc.include_checksum(true)?;
        enc.write_all(&json)?;
        Ok(enc.finish()?)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let json = zstd::stream::decode_all(bytes).context("delta doesn't decompress")?;
        let d: Delta = serde_json::from_slice(&json).context("delta doesn't parse")?;
        ensure!(
            (MIN_FMT..=FMT).contains(&d.fmt),
            "delta format {} isn't supported (this app reads {MIN_FMT}–{FMT})",
            d.fmt
        );
        Ok(d)
    }

    /// Whether the next catalogue is written whole rather than as one more delta, with `since`
    /// deltas since the last whole one, reading that whole one taking `whole` and each delta `each`
    /// (`whole_due`, measured): when the deltas, the next one too, take as long to read as the whole
    /// one, so a reader starting afresh never takes more than about twice one whole catalogue's read.
    /// A run of deltas half as long as MAX_CHAIN, however fast, is due too (readers stop there).
    pub fn due_whole(since: usize, whole: std::time::Duration, each: std::time::Duration) -> bool {
        if since + 1 >= MAX_CHAIN / 2 {
            eprintln!("catalogue: {since} deltas since the last whole catalogue (readers stop at {MAX_CHAIN}): a whole one now");
            return true;
        }
        each * (since as u32 + 1) >= whole
    }
}

/// A catalogue's digest: XXH3-128 of its JSON as written (maps in key order: the same catalogue, the
/// same digest).
pub fn digest(cat: &Catalogue) -> Result<String> {
    Ok(format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&serde_json::to_vec(cat)?)))
}

/// Publishes `cat` into `dir` as what changed over `base` (`<n>.delta.json.zst`), after checking it
/// as `write` does; read back and applied to `base` before it's renamed into place.
pub fn write_delta(dir: &Path, base: &Catalogue, cat: &Catalogue) -> Result<PathBuf> {
    write_delta_of(dir, base, cat, &Delta::between(base, cat)?)
}

/// `write_delta` of a delta made already (`Delta::between`: its size weighed first).
pub fn write_delta_of(dir: &Path, base: &Catalogue, cat: &Catalogue, d: &Delta) -> Result<PathBuf> {
    cat.validate().with_context(|| format!("catalogue {}", cat.n))?;
    let path = dir.join(delta_file_name(cat.n));
    ensure!(!dir.join(file_name(cat.n)).exists(), "catalogue {} exists already", cat.n);
    let check = |b: &[u8]| -> Result<()> {
        let back = Delta::decode(b)?;
        ensure!(&back.apply(base)? == cat, "delta read back doesn't make the catalogue");
        Ok(())
    };
    ensure!(put_once(dir, &path, &d.encode()?, &check)?, "{} appeared meanwhile", path.display());
    Ok(path)
}

/// Whether the next catalogue in `dir` is written whole rather than as a delta: None with no whole
/// catalogue to start a delta from (it's whole anyway); false with no delta since the last whole one;
/// else `Delta::due_whole`, from what reading them costs, measured here and now (task #895, not a
/// figure set by hand): the last whole catalogue read and parsed, and the newest delta read, parsed
/// and applied (to an empty catalogue: what applying one costs is its patch's work). Asked by the
/// catalogue step each time it would write a delta, so a writer that's never idle writes whole ones
/// too.
pub fn whole_due<'a>(dir: impl Into<Loc<'a>>) -> Result<Option<bool>> {
    let dir = dir.into();
    let names = Names::of_dir(dir)?;
    let ns = names.numbers();
    let Some(k) = ns.iter().position(|n| names.whole.contains(n)) else { return Ok(None) };
    if k == 0 {
        return Ok(Some(false));
    }
    // (One that doesn't read, the whole one or the newest delta, makes a whole one due: the chain
    // through it can't be read, so a delta over it would serve nothing. A read that fails stops.)
    let t = std::time::Instant::now();
    let p = dir.join(&file_name(ns[k]));
    if let Err(e) = Catalogue::decode(&p.loc().read().with_context(|| format!("read {}", p.loc().display()))?) {
        eprintln!("catalogue: {} doesn't read ({e:#}): a whole one due", p.loc().display());
        return Ok(Some(true));
    }
    let whole = t.elapsed();
    let t = std::time::Instant::now();
    let p = dir.join(&delta_file_name(ns[0]));
    let d = match Delta::decode(&p.loc().read().with_context(|| format!("read {}", p.loc().display()))?) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("catalogue: {} doesn't read ({e:#}): a whole one due", p.loc().display());
            return Ok(Some(true));
        }
    };
    let _ = delta::apply_in(&mut serde_json::to_value(Catalogue::new(d.of))?, &d.patch);
    let each = t.elapsed();
    Ok(Some(Delta::due_whole(k, whole, each)))
}

/// The catalogues catalogue `n` in `dir` is read through: itself, and back by its deltas' `of` to the
/// whole one they start from (what's kept while `n` is). An error when a delta doesn't read.
pub fn chain<'a>(dir: impl Into<Loc<'a>>, n: u64) -> Result<Vec<u64>> {
    let dir = dir.into();
    let names = Names::of_dir(dir)?;
    let mut out = vec![n];
    let mut at = n;
    while !names.whole.contains(&at) && names.delta.contains(&at) && out.len() <= MAX_CHAIN {
        let p = dir.join(&delta_file_name(at));
        let p = p.loc();
        let d =
            Delta::decode(&p.read().with_context(|| format!("read {}", p.display()))?).with_context(|| p.display())?;
        at = d.of;
        out.push(at);
    }
    Ok(out)
}

/// The number the next publish takes: one past the highest present (damaged ones included, since
/// their names are taken), or 1.
pub fn next_n<'a>(dir: impl Into<Loc<'a>>) -> Result<u64> {
    Ok(list(dir)?.first().map_or(1, |n| n + 1))
}

/// Deletes all but the `keep` highest catalogues in `dir`. For a Mac's own copies
/// only: on the NAS, deletions go through SSH (docs/storage.md); a Mac keeps its copies whole
/// (`write_copy`), so none is a delta another needs. Returns how many were deleted.
pub fn prune(dir: &Path, keep: usize) -> Result<usize> {
    let mut gone = 0;
    for n in list(dir)?.into_iter().skip(keep) {
        for p in [dir.join(file_name(n)), dir.join(delta_file_name(n))] {
            match fs::remove_file(&p) {
                Ok(()) => gone += 1,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("remove {}", p.display())),
            }
        }
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::content_name;
    use serde_json::json;
    use std::time::Duration;

    fn sample(n: u64) -> Catalogue {
        let mut c = Catalogue::new(n);
        c.created = "2026-10-03T04:05:06Z".into();
        c.app = "0.1.0".into();
        let mut add = |logical: &str, ext: &str, size: u64| {
            let h = format!("{:016x}", n * 1000 + size);
            c.files.insert(
                logical.into(),
                FileRef { file: content_name(logical, &h, ext), size, fmt: 1, extra: BTreeMap::new() },
            );
            logical.to_string()
        };
        let root = add("layers/roads/root", "pack", 10);
        let lo = add("layers/roads/lo/3-4-2", "pack", 20);
        let hi = add("layers/roads/hi/6-32-21", "pack", 30);
        let base = add("base/6-32-21", "sect", 40);
        let roads = add("global/roads/6-32-21", "sect", 50);
        let hidata = add("hidata/6-32-21", "sect", 60);
        let pois = add("global/pois", "json", 5);
        let basemap = add("layers/basemap/basemap", "pmtiles", 1000);
        c.units = vec!["6/32/21".into()];
        c.layers.insert(
            "roads".into(),
            Layer {
                encoding: "rt7".into(),
                minzoom: 4,
                maxzoom: 14,
                root: Some(root),
                lo: [("3/4/2".to_string(), lo)].into(),
                hi: [("6/32/21".to_string(), hi)].into(),
                deep: BTreeMap::new(),
                native_zoom: BTreeMap::new(),
                polar: BTreeMap::new(),
                extra: BTreeMap::new(),
            },
        );
        c.basemap = vec![basemap];
        c.base.insert("6/32/21".into(), base);
        c.roads.insert("6/32/21".into(), roads);
        c.hidata.insert("6/32/21".into(), hidata);
        c.global.insert("pois.json".into(), pois);
        c.meta = json!({"bounds": [1, 2, 3, 4]});
        c.credits = json!([
            {"what": "Roads", "source": "OpenStreetMap", "terms": "© OpenStreetMap contributors, ODbL"},
            {"what": "Road elevation, Japan", "source": "GSI Tiles", "terms": "PDL 1.0", "areas": [[122.5, 20.0, 154.0, 46.5]]}
        ]);
        c.coverage = json!({"regions": [{"id": "northumberland", "name": "Northumberland", "outline": ["osm:88066"],
            "shapes": {"osm:88066": [[[[-2.68941, 55.80271], [-1.46055, 55.07428], [-2.56493, 54.78346], [-2.68941, 55.80271]]]]}}]});
        c
    }

    #[test]
    fn round_trip_keeps_unknown_fields() {
        let c = sample(7);
        c.validate().unwrap();
        assert_eq!(Catalogue::decode(&c.encode().unwrap()).unwrap(), c);
        assert_eq!(c.encode().unwrap(), c.encode().unwrap(), "deterministic");

        // A newer publisher's extra fields survive a read and a write.
        let mut v = serde_json::to_value(&c).unwrap();
        v["future"] = json!({"x": 1});
        v["files"]["global/pois"]["etag"] = json!("abc");
        v["layers"]["roads"]["tilesize"] = json!(512);
        let c2: Catalogue = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(c2.extra["future"], json!({"x": 1}));
        assert_eq!(serde_json::to_value(&c2).unwrap(), v);

        // Missing optional parts are fine; missing fmt or n isn't.
        let minimal: Catalogue = serde_json::from_str(r#"{"fmt":1,"n":3}"#).unwrap();
        assert_eq!(minimal.n, 3);
        assert!(serde_json::from_str::<Catalogue>(r#"{"fmt":1}"#).is_err());
    }

    #[test]
    fn validate_catches_dangling_references() {
        let mut c = sample(1);
        c.base.insert("6/0/0".into(), "base/6-0-0".into());
        assert!(c.validate().unwrap_err().to_string().contains("base/6-0-0"));
        let mut c = sample(1);
        c.files.get_mut("global/pois").unwrap().file = "global/other.0123456789abcdef.json".into();
        assert!(c.validate().is_err());
        let mut c = sample(1);
        c.files.get_mut("global/pois").unwrap().file = "global/pois.json".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn write_latest_and_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("catalog");
        assert!(latest(&d).unwrap().is_none());
        assert_eq!(next_n(&d).unwrap(), 1);

        for n in 1..=3 {
            let p = write(&d, &sample(n)).unwrap();
            assert_eq!(p, d.join(format!("{n}.json.zst")));
        }
        assert_eq!(next_n(&d).unwrap(), 4);
        assert_eq!(latest(&d).unwrap().unwrap(), sample(3));
        assert!(!d.join("3.json.zst.tmp").exists());

        // Writing the same catalogue again is a no-op; different content under a used number is not.
        write(&d, &sample(3)).unwrap();
        let mut other = sample(3);
        other.app = "0.2.0".into();
        assert!(write(&d, &other).is_err());
        assert_eq!(latest(&d).unwrap().unwrap(), sample(3));

        // A truncated newest catalogue (a copy cut short) is skipped for the one before.
        let good4 = sample(4).encode().unwrap();
        fs::write(d.join("4.json.zst"), &good4[..good4.len() - 3]).unwrap();
        assert_eq!(latest(&d).unwrap().unwrap().n, 3);
        // So is garbage, and a file whose number disagrees with its content.
        fs::write(d.join("5.json.zst"), b"not zstd").unwrap();
        fs::write(d.join("6.json.zst"), sample(2).encode().unwrap()).unwrap();
        // And a catalogue in a format this app doesn't read.
        // (Task #449: format 1, before markdata bundles, still read.)
        let mut one = sample(1);
        one.fmt = 1;
        assert_eq!(Catalogue::decode(&one.encode().unwrap()).unwrap().fmt, 1);
        let mut future = sample(7);
        future.fmt = FMT + 1;
        fs::write(d.join("7.json.zst"), future.encode().unwrap()).unwrap();
        // Unrelated and in-progress files are ignored.
        fs::write(d.join("8.json.zst.tmp"), b"partial").unwrap();
        fs::write(d.join("notes.txt"), b"x").unwrap();
        assert_eq!(latest(&d).unwrap().unwrap().n, 3);
        assert_eq!(next_n(&d).unwrap(), 8);

        // A flipped byte inside the frame is caught by zstd's checksum.
        let mut bad = sample(9).encode().unwrap();
        let mid = bad.len() / 2;
        bad[mid] ^= 0x01;
        fs::write(d.join("9.json.zst"), &bad).unwrap();
        assert_eq!(latest(&d).unwrap().unwrap().n, 3);

        assert_eq!(prune(&d, 2).unwrap(), 6);
        assert_eq!(list(&d).unwrap(), [9, 7]);
    }

    #[test]
    fn latest_through_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("catalog");
        let pool = IoPool::new(2, Duration::from_secs(60), dir.path().to_owned());
        let nd = crate::nasfs::NasRoot::local_for_test(dir.path()).join("catalog");
        assert!(latest_nas(&pool, &nd).unwrap().is_none());
        assert!(latest_on(&nd).unwrap().is_none());
        write(&d, &sample(1)).unwrap();
        write(&d, &sample(2)).unwrap();
        fs::write(d.join("3.json.zst"), b"damaged").unwrap();
        assert_eq!(latest_nas(&pool, &nd).unwrap().unwrap().n, 2);
        assert_eq!(latest_on(&nd).unwrap().unwrap().n, 2);
        // Offline is an error, not "no catalogue".
        pool.mark_offline("test");
        let e = latest_nas(&pool, &nd).unwrap_err();
        assert!(matches!(IoError::find(&e), Some(IoError::Offline)), "{e:#}");
    }

    /// Catalogue `n` of a run: each some tiles drawn again, a unit added, a region's shapes redrawn or
    /// a region added, now and then one taken away (as publishing changes a catalogue).
    fn evolve(prev: &Catalogue, n: u64) -> Catalogue {
        let mut c = prev.clone();
        c.n = n;
        c.created = format!("2026-10-15T{:02}:00:00Z", n % 24);
        let add = |c: &mut Catalogue, logical: &str, ext: &str| {
            let h = format!("{:016x}", n * 7919 + logical.len() as u64);
            c.files.insert(
                logical.into(),
                FileRef { file: content_name(logical, &h, ext), size: n, fmt: 1, extra: BTreeMap::new() },
            );
            logical.to_string()
        };
        let hi = add(&mut c, &format!("layers/roads/hi/6-{}-21", 30 + n % 5), "pack");
        c.layers.get_mut("roads").unwrap().hi.insert(format!("6/{}/21", 30 + n % 5), hi);
        let base = add(&mut c, &format!("base/6-{n}-9"), "sect");
        c.base.insert(format!("6/{n}/9"), base);
        c.units.push(format!("6/{n}/9"));
        if n % 4 == 0 {
            // (A unit the coverage no longer builds: gone from the catalogue.)
            let u = format!("6/{}/9", n - 2);
            c.units.retain(|x| *x != u);
            c.base.remove(&u);
            c.files.remove(&format!("base/6-{}-9", n - 2));
        }
        let regions = c.coverage["regions"].as_array_mut().unwrap();
        match n % 3 {
            0 => regions.push(json!({"id": format!("r{n}"), "name": format!("R{n}"), "outline": [format!("osm:{n}")], "shapes": {format!("osm:{n}"): [[[[n, 1], [n, 2], [n + 1, 2], [n, 1]]]]}})),
            1 => regions[0]["shapes"]["osm:88066"][0][0][0] = json!([n, n]),
            _ if regions.len() > 2 => {
                regions.remove(1);
            }
            _ => {}
        }
        c.meta["ways"] = json!(n * 100);
        c
    }

    #[test]
    fn a_catalogue_read_through_its_deltas_is_the_whole_one_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let (d, whole) = (dir.path().join("catalog"), dir.path().join("whole"));
        let mut prev = sample(1);
        write(&d, &prev).unwrap();
        write(&whole, &prev).unwrap();
        for n in 2..=12 {
            let c = evolve(&prev, n);
            // Every third whole, the rest as deltas over the one before (read whole, as the writer does).
            if n % 3 == 0 {
                write(&d, &c).unwrap();
            } else {
                let base = read_n(&d, n - 1).unwrap();
                let p = write_delta(&d, &base, &c).unwrap();
                assert_eq!(p, d.join(format!("{n}.delta.json.zst")));
            }
            write(&whole, &c).unwrap();
            prev = c;
        }
        assert_eq!(list(&d).unwrap(), (1..=12).rev().collect::<Vec<_>>());
        assert_eq!(next_n(&d).unwrap(), 13);
        // Every catalogue by number, and the newest, as the whole ones written beside.
        for n in 1..=12 {
            assert_eq!(read_n(&d, n).unwrap(), read_n(&whole, n).unwrap(), "catalogue {n}");
        }
        assert_eq!(latest(&d).unwrap().unwrap(), latest(&whole).unwrap().unwrap());
        let pool = IoPool::new(2, Duration::from_secs(60), dir.path().to_owned());
        let nd = crate::nasfs::NasRoot::local_for_test(dir.path()).join("catalog");
        assert_eq!(latest_nas(&pool, &nd).unwrap().unwrap(), prev);
        assert_eq!(latest_on(&nd).unwrap().unwrap(), prev);
        // From one this Mac has: the deltas after it applied to it; itself when it's the newest.
        let have = read_n(&whole, 10).unwrap();
        assert_eq!(latest_nas_from(&pool, &nd, Some(&have)).unwrap().unwrap(), prev);
        assert_eq!(latest_nas_from(&pool, &nd, Some(&prev)).unwrap().unwrap(), prev);
        // The chains: back to the whole one each starts from.
        assert_eq!(chain(&d, 11).unwrap(), [11, 10, 9]);
        assert_eq!(chain(&d, 9).unwrap(), [9]);
        assert_eq!(whole_due(&d).unwrap(), Some(false));
        assert_eq!(whole_due(&dir.path().join("none")).unwrap(), None);
        // A delta is smaller than the whole catalogue it makes.
        assert!(
            fs::metadata(d.join("11.delta.json.zst")).unwrap().len()
                < fs::metadata(whole.join("11.json.zst")).unwrap().len() / 2
        );
        // An older app's reader lists only the whole ones (its names): it keeps to catalogue 12's
        // last whole one, 12 itself here.
        assert!(fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter_map(|e| number(e.file_name().to_str()?))
            .all(|n| n % 3 == 0 || n == 1));
    }

    /// Task #684: from one read before, only the deltas after it are read: the chain behind it
    /// gone, it still reads.
    #[test]
    fn read_n_from_one_read_before_reads_only_the_deltas_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("catalog");
        let one = sample(1);
        write(&d, &one).unwrap();
        let two = evolve(&one, 2);
        write_delta(&d, &one, &two).unwrap();
        let have = read_n(&d, 2).unwrap();
        let three = evolve(&two, 3);
        write_delta(&d, &two, &three).unwrap();
        fs::remove_file(d.join(file_name(1))).unwrap();
        assert!(read_n(&d, 3).is_err(), "back to the whole one: gone");
        assert_eq!(read_n_from(&d, 3, Some(&have)).unwrap(), three);
        assert_eq!(read_n_from(&d, 2, Some(&have)).unwrap(), have, "itself, by its delta's digest");
        // (A number taken again: the one read before isn't the catalogue numbered so now, so it's
        // read without it, not served stale.)
        let other = tempfile::tempdir().unwrap();
        let o = other.path().join("catalog");
        let mut alt = sample(1);
        alt.app = "elsewhere".into();
        write(&o, &alt).unwrap();
        let alt2 = evolve(&alt, 2);
        write_delta(&o, &alt, &alt2).unwrap();
        assert_eq!(read_n_from(&o, 2, Some(&have)).unwrap(), alt2, "not the one read before");
        let alt3 = evolve(&alt2, 3);
        write_delta(&o, &alt2, &alt3).unwrap();
        assert_eq!(read_n_from(&o, 3, Some(&have)).unwrap(), alt3, "its delta doesn't fit: read whole");
    }

    #[test]
    fn a_delta_that_doesnt_fit_is_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("catalog");
        let one = sample(1);
        write(&d, &one).unwrap();
        let two = evolve(&one, 2);
        write_delta(&d, &one, &two).unwrap();
        let three = evolve(&two, 3);
        write_delta(&d, &two, &three).unwrap();
        // Applied to another catalogue 2 than the one it changes: refused by its digest.
        let mut other = two.clone();
        other.app = "else".into();
        let d3 = Delta::decode(&fs::read(d.join("3.delta.json.zst")).unwrap()).unwrap();
        assert!(d3.apply(&other).unwrap_err().to_string().contains("digests differ"));
        assert!(d3.apply(&one).is_err());
        // A damaged delta: the newest that reads whole instead.
        let mut bad = fs::read(d.join("3.delta.json.zst")).unwrap();
        let mid = bad.len() / 2;
        bad[mid] ^= 1;
        fs::write(d.join("3.delta.json.zst"), &bad).unwrap();
        assert_eq!(latest(&d).unwrap().unwrap(), two);
        // The whole one under it gone: none of them reads.
        fs::remove_file(d.join("3.delta.json.zst")).unwrap();
        fs::remove_file(d.join("1.json.zst")).unwrap();
        assert!(latest(&d).unwrap().is_none());
        assert!(read_n(&d, 2).is_err());
        // A number is taken once, whole or as a delta.
        write(&d, &one).unwrap();
        assert!(write(&d, &two).is_err());
        assert!(write_delta(&d, &one, &one).is_err());
    }

    #[test]
    fn whole_again_once_the_deltas_take_as_long_as_a_whole_one() {
        let ms = Duration::from_millis;
        assert!(!Delta::due_whole(0, ms(100), ms(10)));
        assert!(!Delta::due_whole(8, ms(100), ms(10)));
        assert!(Delta::due_whole(9, ms(100), ms(10)));
        assert!(Delta::due_whole(0, ms(100), ms(100)));
        // (However fast, before the readers' limit.)
        assert!(Delta::due_whole(MAX_CHAIN / 2 - 1, Duration::MAX, ms(0)));
        assert!(!Delta::due_whole(MAX_CHAIN / 2 - 2, Duration::MAX, ms(0)));
    }

    /// A whole catalogue due once reading the deltas past the newest whole one takes as long as it
    /// (measured: some time before the readers' limit); none to start a delta from with no whole one.
    #[test]
    fn whether_a_whole_one_is_due() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("catalog");
        assert_eq!(whole_due(&d).unwrap(), None);
        let one = sample(1);
        write(&d, &one).unwrap();
        assert_eq!(whole_due(&d).unwrap(), Some(false));
        let mut prev = one;
        let mut n = 2;
        while whole_due(&d).unwrap() == Some(false) {
            assert!((n as usize) < MAX_CHAIN / 2 + 2, "never due");
            let next = evolve(&prev, n);
            write_delta(&d, &prev, &next).unwrap();
            prev = next;
            n += 1;
        }
        assert!(n > 2, "due with no delta");
        let whole = evolve(&prev, n);
        write(&d, &whole).unwrap();
        assert_eq!(whole_due(&d).unwrap(), Some(false));
        // The newest delta damaged: a whole one due (the chain through it doesn't read), not an error.
        write_delta(&d, &whole, &evolve(&whole, n + 1)).unwrap();
        fs::write(d.join(delta_file_name(n + 1)), b"torn").unwrap();
        assert_eq!(whole_due(&d).unwrap(), Some(true));
    }

    /// Catalogue `n` of `dir` as read before task #895: its chain from the whole one, each delta
    /// applied by `Delta::apply` (the catalogue to JSON and back, both digests checked, at each).
    fn stepwise(dir: &Path, n: u64) -> Catalogue {
        let ch = chain(dir, n).unwrap();
        let mut c = read(&dir.join(file_name(*ch.last().unwrap()))).unwrap();
        for &k in ch.iter().rev().skip(1) {
            c = Delta::decode(&fs::read(dir.join(delta_file_name(k))).unwrap()).unwrap().apply(&c).unwrap();
        }
        c
    }

    /// A recorded chain of 40 deltas: every catalogue on it reads as it did delta by delta (and as
    /// it was written), byte for byte.
    #[test]
    fn a_chain_on_one_value_is_the_chain_delta_by_delta() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("catalog");
        let mut prev = sample(1);
        write(&d, &prev).unwrap();
        let mut made = vec![prev.clone()];
        for n in 2..=41 {
            let c = evolve(&prev, n);
            write_delta(&d, &prev, &c).unwrap();
            made.push(c.clone());
            prev = c;
        }
        for c in &made {
            let got = read_n(&d, c.n).unwrap();
            assert_eq!(serde_json::to_vec(&got).unwrap(), serde_json::to_vec(&stepwise(&d, c.n)).unwrap());
            assert_eq!(&got, c);
        }
        assert_eq!(latest(&d).unwrap().unwrap(), prev);
    }

    /// A delta in the middle of a chain that makes another catalogue than it names: the chain isn't
    /// read, and the bisection names that delta.
    #[test]
    fn a_wrong_delta_in_a_chain_is_found() {
        let mut base = sample(1);
        base.app = "x".into();
        let mut chain = Vec::new();
        let mut prev = base.clone();
        for n in 2..=20 {
            let c = evolve(&prev, n);
            chain.push(Delta::between(&prev, &c).unwrap());
            prev = c;
        }
        // (Delta 9's patch changed: what it and every later one make differs from what they name.)
        chain[7].patch["o"]["app"] = json!({"=": "y"});
        assert!(first_wrong(&base, &chain).starts_with("delta 9 over 8:"), "{}", first_wrong(&base, &chain));
        // In a folder, a delta that says it makes another catalogue than it does.
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("catalog");
        write(&d, &base).unwrap();
        let mut prev = base.clone();
        for n in 2..=12 {
            let mut c = evolve(&prev, n);
            if n == 6 {
                c.app = "y".into();
            }
            let mut dl = Delta::between(&prev, &c).unwrap();
            if n == 6 {
                // (It says it makes the catalogue without the change.)
                dl.digest = digest(&evolve(&prev, n)).unwrap();
            }
            fs::write(d.join(delta_file_name(n)), dl.encode().unwrap()).unwrap();
            prev = c;
        }
        // (Every catalogue from 6 on is read through it: none reads; 5 is the newest that does.)
        assert_eq!(latest(&d).unwrap().unwrap().n, 5);
    }

    /// With `keep_read_in`, a catalogue made from deltas is kept here, and the next read of its
    /// chain starts from it: the whole catalogue under it, and the deltas up to it, aren't read.
    #[test]
    fn a_catalogue_made_from_deltas_is_kept_for_the_next_read() {
        keep_read_in(tempfile::tempdir().unwrap().keep());
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("catalog");
        let mut prev = sample(1);
        prev.app = "kept".into();
        write(&d, &prev).unwrap();
        for n in 2..=10 {
            let c = evolve(&prev, n);
            write_delta(&d, &prev, &c).unwrap();
            prev = c;
        }
        assert_eq!(latest(&d).unwrap().unwrap(), prev);
        assert!(kept_in().unwrap().join(kept_name(10, &digest(&prev).unwrap())).exists());
        // The whole one and the deltas before 10 gone: 10 and the two after it still read.
        fs::remove_file(d.join(file_name(1))).unwrap();
        for n in 2..10 {
            fs::remove_file(d.join(delta_file_name(n))).unwrap();
        }
        assert_eq!(read_n(&d, 10).unwrap(), prev);
        // A kept file cut short (a torn write) or changed isn't used, and is written again.
        let k10 = kept_in().unwrap().join(kept_name(10, &digest(&prev).unwrap()));
        let good = fs::read(&k10).unwrap();
        fs::write(&k10, &good[..good.len() / 2]).unwrap();
        assert!(kept(10, &digest(&prev).unwrap()).is_none());
        let mut other10 = prev.clone();
        other10.app = "changed".into();
        let mut enc = zstd::stream::Encoder::new(Vec::new(), 1).unwrap();
        enc.write_all(&serde_json::to_vec(&other10).unwrap()).unwrap();
        fs::write(&k10, enc.finish().unwrap()).unwrap();
        assert!(kept(10, &digest(&prev).unwrap()).is_none());
        // (Its chain on the NAS again: read from it, and kept whole once more.)
        let t3 = tempfile::tempdir().unwrap();
        let d3 = t3.path().join("catalog");
        let mut p3 = sample(1);
        p3.app = "kept".into();
        write(&d3, &p3).unwrap();
        for n in 2..=10 {
            let c = evolve(&p3, n);
            write_delta(&d3, &p3, &c).unwrap();
            p3 = c;
        }
        assert_eq!(p3, prev);
        assert_eq!(read_n(&d3, 10).unwrap(), prev);
        assert_eq!(fs::read(&k10).unwrap(), good);
        // A temporary file an hour old (a writer stopped mid-write) is swept by the next keep.
        let stale = kept_in().unwrap().join("9-x.json.zst.host-1.tmp.0");
        fs::write(&stale, b"torn").unwrap();
        File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(7200))
            .unwrap();
        let fresh = kept_in().unwrap().join("9-y.json.zst.host-2.tmp.0");
        fs::write(&fresh, b"under way").unwrap();
        for n in 11..=12 {
            let c = evolve(&prev, n);
            write_delta(&d, &prev, &c).unwrap();
            prev = c;
        }
        assert_eq!(latest(&d).unwrap().unwrap(), prev);
        assert!(!stale.exists() && fresh.exists());
        // A kept one that isn't the one a delta changes (its number taken again): not used.
        let t2 = tempfile::tempdir().unwrap();
        let d2 = t2.path().join("catalog");
        let mut other = sample(1);
        other.app = "other".into();
        write(&d2, &other).unwrap();
        let mut p2 = other;
        for n in 2..=11 {
            let c = evolve(&p2, n);
            write_delta(&d2, &p2, &c).unwrap();
            p2 = c;
        }
        assert_eq!(latest(&d2).unwrap().unwrap(), p2);
        assert_ne!(p2, prev);
    }

    /// The real chain (task #895): `SCENIC_CATALOGUE_CHAIN=<a catalogue folder>` reads its newest
    /// catalogue, prints the time it took and its digest, and checks it is, byte for byte, what
    /// the chain makes delta by delta as before (none set: nothing runs).
    #[test]
    fn real_chain_time_the_newest() {
        let Ok(dir) = std::env::var("SCENIC_CATALOGUE_CHAIN") else { return };
        let dir = Path::new(&dir);
        // (`SCENIC_CATALOGUE_KEEP=<folder>`: kept there, `keep_read_in`, as the apps do; run twice,
        // the second is a restart's read.)
        if let Some(k) = std::env::var_os("SCENIC_CATALOGUE_KEEP") {
            keep_read_in(PathBuf::from(k));
        }
        let t = std::time::Instant::now();
        let c = latest(dir).unwrap().unwrap();
        println!("catalogue {} read in {:.1} s, digest {}", c.n, t.elapsed().as_secs_f64(), digest(&c).unwrap());
        if std::env::var_os("SCENIC_CATALOGUE_STEPWISE").is_some() {
            let t = std::time::Instant::now();
            let s = stepwise(dir, c.n);
            println!("delta by delta: {:.1} s", t.elapsed().as_secs_f64());
            assert_eq!(serde_json::to_vec(&s).unwrap(), serde_json::to_vec(&c).unwrap());
            println!("the same bytes");
        }
    }
}
