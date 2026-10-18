//! Catalogs (docs/formats.md, Catalog): the only index of what's on the NAS. Each publish writes
//! the next `catalog/<n>.json.zst` (zstd with its content checksum on), naming every current file
//! by content name with its size and format version. Readers take the highest `<n>` that decodes
//! and parses, falling back past a damaged or half-copied one. A Mac keeps copies of the last few
//! it read in its own `catalog/`, for starting offline.

use crate::iopool::{IoError, IoPool};
use crate::naming::{parse_content_name, rename_no_replace, tmp_path};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The catalog format version written.
pub const FMT: u32 = 1;
/// The oldest version read (plan §8).
pub const MIN_FMT: u32 = 1;

const LEVEL: i32 = 9;
const SUFFIX: &str = ".json.zst";
const DELTA_SUFFIX: &str = ".delta.json.zst";

pub mod delta;

/// One catalog (docs/formats.md). Maps are `BTreeMap`s so the JSON comes out the same for the same
/// catalog; fields this version doesn't know are kept in `extra`, so nothing is lost when a
/// catalog is read and written again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// The catalog format version.
    pub fmt: u32,
    /// The publish number, also the file's name.
    pub n: u64,
    /// RFC 3339 time of the publish.
    #[serde(default)]
    pub created: String,
    /// The published app that made it (its folder's name), or "development".
    #[serde(default)]
    pub app: String,
    /// Every file the catalog references, by logical name. GC keeps them, and the files of the last
    /// 14 days' catalogs (docs/plan.md §3).
    #[serde(default)]
    pub files: BTreeMap<String, FileRef>,
    /// The built units (tile keys as "z/x/y"), sorted.
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
    /// (pipeline::rules::Credit). Empty in catalogs made before they were recorded. Kept as JSON,
    /// as `coverage` is, so a part the app can't read never makes the catalog unreadable.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub credits: Value,
    /// The coverage it was built for: `{"regions": [{"id", "name", "outline", "shapes"}]}`, each
    /// outline entry's polygons simplified for drawing (pipeline::coverage::DrawnRegion). No
    /// regions in catalogs made before they were recorded.
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
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Catalog {
    /// An empty catalog number `n` in the current format.
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

    /// Every logical name the catalog's sections refer to, with where it's referred from.
    pub fn references(&self) -> Vec<(String, &str)> {
        let mut out = Vec::new();
        for (name, l) in &self.layers {
            if let Some(r) = &l.root {
                out.push((format!("layers.{name}.root"), r.as_str()));
            }
            for (k, v) in l.lo.iter().chain(&l.hi) {
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
        ensure!(self.fmt == FMT, "catalog format {} (this app writes {FMT})", self.fmt);
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

    /// The catalog as stored: compact JSON, zstd level 9 with its content checksum.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(self)?;
        let mut enc = zstd::stream::Encoder::new(Vec::with_capacity(json.len() / 4 + 64), LEVEL)?;
        enc.include_checksum(true)?;
        enc.write_all(&json)?;
        Ok(enc.finish()?)
    }

    /// Decodes a stored catalog: an error when the zstd frame is damaged or cut short (the checksum
    /// catches what the format doesn't), the JSON doesn't parse, or the format isn't one this app
    /// reads.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let json = zstd::stream::decode_all(bytes).context("catalog doesn't decompress")?;
        let c: Catalog = serde_json::from_slice(&json).context("catalog doesn't parse")?;
        ensure!(
            (MIN_FMT..=FMT).contains(&c.fmt),
            "catalog format {} isn't supported (this app reads {MIN_FMT}–{FMT})",
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

/// The file name of catalog `n`, whole.
pub fn file_name(n: u64) -> String {
    format!("{n}{SUFFIX}")
}

/// The file name of catalog `n` written as what changed over the one before it (`Delta`).
pub fn delta_file_name(n: u64) -> String {
    format!("{n}{DELTA_SUFFIX}")
}

/// Publishes `cat` into `dir` as `<n>.json.zst` after checking it (`Catalog::validate`).
pub fn write(dir: &Path, cat: &Catalog) -> Result<PathBuf> {
    cat.validate().with_context(|| format!("catalog {}", cat.n))?;
    ensure!(!dir.join(delta_file_name(cat.n)).exists(), "catalog {} exists already, as a delta", cat.n);
    write_copy(dir, cat)
}

/// Writes `cat` into `dir` as `<n>.json.zst`, unchecked: for keeping a copy of a catalog read
/// elsewhere (a Mac's own `catalog/`). The bytes go to `<n>.json.zst.tmp`, are decoded back and
/// compared, then renamed into place. A catalog number is written once: if it exists with the
/// same content this does nothing, and with other content it's an error.
pub fn write_copy(dir: &Path, cat: &Catalog) -> Result<PathBuf> {
    let path = dir.join(file_name(cat.n));
    if let Some(done) = same_as_existing(&path, cat)? {
        return Ok(done);
    }
    let bytes = cat.encode()?;
    let check = |b: &[u8]| -> Result<()> {
        let back = Catalog::decode(b)?;
        ensure!(&back == cat, "catalog read back differs from what was written");
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

/// `Some(path)` when the catalog file exists and holds `cat`; None when it doesn't exist; an error
/// when it holds something else.
fn same_as_existing(path: &Path, cat: &Catalog) -> Result<Option<PathBuf>> {
    match fs::read(path) {
        Ok(b) => match Catalog::decode(&b) {
            Ok(old) if &old == cat => Ok(Some(path.to_owned())),
            Ok(_) => bail!("{} exists with other content; a catalog number is never reused", path.display()),
            Err(e) => bail!("{} exists but is damaged ({e:#}); it must be removed by hand", path.display()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Reads one whole catalog file (`read_n` reads a catalog by number, whichever way it was written).
pub fn read(path: &Path) -> Result<Catalog> {
    let b = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Catalog::decode(&b).with_context(|| format!("{}", path.display()))
}

/// Catalog `n` in `dir`, whole: its own file, or what changed over the one before it applied to
/// that one (`Delta`), back to a whole one.
pub fn read_n(dir: &Path, n: u64) -> Result<Catalog> {
    let names = Names::of_dir(dir)?;
    materialize(dir, &names, n, None, &mut local_read)?
        .with_context(|| format!("catalog {n} in {} can't be read whole", dir.display()))
}

/// The file catalog `n` is in (whole, or as a delta), for its time: None when neither is there.
pub fn path_n(dir: &Path, n: u64) -> Option<PathBuf> {
    [file_name(n), delta_file_name(n)].into_iter().map(|f| dir.join(f)).find(|p| p.exists())
}

/// The catalog numbers in `dir`, whole or deltas, highest first (none when `dir` doesn't exist).
pub fn list(dir: &Path) -> Result<Vec<u64>> {
    Ok(Names::of_dir(dir)?.numbers())
}

/// The catalogs in a folder by kind.
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

    fn of_dir(dir: &Path) -> Result<Self> {
        let mut names = Names::default();
        let rd = match fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(names),
            Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
        };
        for e in rd {
            let e = e.with_context(|| format!("list {}", dir.display()))?;
            if let Some(name) = e.file_name().to_str() {
                names.add(name);
            }
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

fn local_read(p: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            eprintln!("catalog: skipping {}: {e}", p.display());
            Ok(None)
        }
    }
}

/// The highest catalog in `dir` that reads whole; damaged ones are logged and skipped.
pub fn latest(dir: &Path) -> Result<Option<Catalog>> {
    let names = Names::of_dir(dir)?;
    pick(dir, &names, None, &mut local_read)
}

/// `latest` for a directory on the NAS: every listing and read goes through the I/O pool. The
/// NAS going offline midway is an error, never a reason to fall back to an older catalog.
pub fn latest_nas(pool: &IoPool, dir: &Path) -> Result<Option<Catalog>> {
    latest_nas_from(pool, dir, None)
}

/// `latest_nas` from a catalog this Mac read before (`have`): the newest is `have` itself when its
/// number is the highest there (nothing read but the listing), and the deltas after it are read
/// and applied to it, not the chain back to a whole catalog.
pub fn latest_nas_from(pool: &IoPool, dir: &Path, have: Option<&Catalog>) -> Result<Option<Catalog>> {
    let mut names = Names::default();
    match pool.list(dir) {
        Ok(items) => {
            for i in items.iter().filter(|i| !i.is_dir) {
                names.add(&i.name);
            }
        }
        Err(IoError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
    }
    if let Some(h) = have.filter(|h| names.numbers().first() == Some(&h.n)) {
        return Ok(Some(h.clone()));
    }
    pick(dir, &names, have, &mut |p| match pool.read_all(p) {
        Ok(b) => Ok(Some(b)),
        Err(IoError::Io(e)) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(IoError::Io(e)) => {
            eprintln!("catalog: skipping {}: {e}", p.display());
            Ok(None)
        }
        Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
    })
}

/// The first catalog of `names` (highest first) that reads whole.
fn pick(
    dir: &Path,
    names: &Names,
    have: Option<&Catalog>,
    read: &mut dyn FnMut(&Path) -> Result<Option<Vec<u8>>>,
) -> Result<Option<Catalog>> {
    for n in names.numbers() {
        if let Some(c) = materialize(dir, names, n, have, read)? {
            return Ok(Some(c));
        }
    }
    Ok(None)
}

/// Catalog `n` whole: its own file, or its delta applied to the one before it (`have` when that's
/// the one, else read the same way), each step checked by the digests the delta carries. None
/// (logged) when a file is damaged, missing or doesn't carry its own number, or a digest differs.
fn materialize(
    dir: &Path,
    names: &Names,
    n: u64,
    have: Option<&Catalog>,
    read: &mut dyn FnMut(&Path) -> Result<Option<Vec<u8>>>,
) -> Result<Option<Catalog>> {
    let mut chain: Vec<Delta> = Vec::new();
    let mut at = n;
    let base = loop {
        if let Some(h) = have.filter(|h| h.n == at) {
            break h.clone();
        }
        if chain.len() > MAX_CHAIN {
            eprintln!("catalog: skipping {n}: its deltas go back more than {MAX_CHAIN}");
            return Ok(None);
        }
        if names.whole.contains(&at) {
            let p = dir.join(file_name(at));
            let Some(bytes) = read(&p)? else { return Ok(None) };
            match Catalog::decode(&bytes) {
                Ok(c) if c.n == at => break c,
                Ok(c) => eprintln!("catalog: skipping {}: it says it is number {}", p.display(), c.n),
                Err(e) => eprintln!("catalog: skipping {}: {e:#}", p.display()),
            }
            return Ok(None);
        }
        if !names.delta.contains(&at) {
            eprintln!("catalog: skipping {n}: catalog {at}, which it changes, isn't there");
            return Ok(None);
        }
        let p = dir.join(delta_file_name(at));
        let Some(bytes) = read(&p)? else { return Ok(None) };
        match Delta::decode(&bytes) {
            Ok(d) if d.n == at && d.of < at => {
                at = d.of;
                chain.push(d);
            }
            Ok(d) => {
                eprintln!("catalog: skipping {}: it says it is number {} over {}", p.display(), d.n, d.of);
                return Ok(None);
            }
            Err(e) => {
                eprintln!("catalog: skipping {}: {e:#}", p.display());
                return Ok(None);
            }
        }
    };
    if chain.len() >= MAX_CHAIN / 2 {
        eprintln!(
            "catalog: {n} is read through {} deltas (at most {MAX_CHAIN}): its writer should have written a whole one",
            chain.len()
        );
    }
    let mut cat = base;
    for d in chain.iter().rev() {
        match d.apply(&cat) {
            Ok(c) => cat = c,
            Err(e) => {
                eprintln!("catalog: skipping {n}: delta {} over {}: {e:#}", d.n, d.of);
                return Ok(None);
            }
        }
    }
    Ok(Some(cat))
}

/// The longest run of deltas read back to a whole catalog (a writer writes a whole one long
/// before: `Delta::due_whole`).
const MAX_CHAIN: usize = 1000;

/// A catalog written as what changed over the one before it (docs/formats.md, Delta catalogs):
/// `catalog/<n>.delta.json.zst`, zstd with its content checksum. This app's readers apply it to
/// catalog `of` (whole, or itself a delta) and get catalog `n` exactly as the whole one would be:
/// `of_digest` and `digest` (`digest`) check both ends. An older app's readers don't list it (its
/// name isn't `<n>.json.zst`): they keep to the last whole catalog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub fmt: u32,
    pub n: u64,
    /// The catalog it changes.
    pub of: u64,
    pub of_digest: String,
    /// Catalog `n`'s digest, whole.
    pub digest: String,
    /// `delta::diff` of the two as JSON.
    pub patch: Value,
}

impl Delta {
    /// The delta from `base` to `cat`.
    pub fn between(base: &Catalog, cat: &Catalog) -> Result<Self> {
        let patch = delta::diff(&serde_json::to_value(base)?, &serde_json::to_value(cat)?);
        Ok(Delta { fmt: FMT, n: cat.n, of: base.n, of_digest: digest(base)?, digest: digest(cat)?, patch })
    }

    /// Catalog `n`: this applied to `base` (catalog `of`), checked by the digests.
    pub fn apply(&self, base: &Catalog) -> Result<Catalog> {
        ensure!(base.n == self.of, "it changes catalog {}, not {}", self.of, base.n);
        ensure!(digest(base)? == self.of_digest, "catalog {} isn't the one it changes (digests differ)", base.n);
        let v = delta::apply(&serde_json::to_value(base)?, &self.patch)?;
        let c: Catalog = serde_json::from_value(v).context("the patched catalog doesn't parse")?;
        ensure!(c.n == self.n && digest(&c)? == self.digest, "applied, it isn't catalog {} (digests differ)", self.n);
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

    /// Whether the next catalog is written whole rather than as a delta of `size` bytes: when the
    /// deltas since the last whole one (`since`, their sizes) and it add up to the whole one's size
    /// (`whole`) or more, so a reader starting afresh never reads more than two whole catalogs' worth.
    /// A run of deltas as long as MAX_CHAIN, whatever their sizes, is due too (readers stop there).
    pub fn due_whole(since: &[u64], size: u64, whole: u64) -> bool {
        if since.len() + 1 >= MAX_CHAIN / 2 {
            eprintln!(
                "catalog: {} deltas since the last whole catalog (readers stop at {MAX_CHAIN}): a whole one now",
                since.len()
            );
            return true;
        }
        since.iter().sum::<u64>() + size >= whole
    }
}

/// A catalog's digest: XXH3-128 of its JSON as written (maps in key order: the same catalog, the
/// same digest).
pub fn digest(cat: &Catalog) -> Result<String> {
    Ok(format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&serde_json::to_vec(cat)?)))
}

/// Publishes `cat` into `dir` as what changed over `base` (`<n>.delta.json.zst`), after checking it
/// as `write` does; read back and applied to `base` before it's renamed into place.
pub fn write_delta(dir: &Path, base: &Catalog, cat: &Catalog) -> Result<PathBuf> {
    write_delta_of(dir, base, cat, &Delta::between(base, cat)?)
}

/// `write_delta` of a delta made already (`Delta::between`: its size weighed first).
pub fn write_delta_of(dir: &Path, base: &Catalog, cat: &Catalog, d: &Delta) -> Result<PathBuf> {
    cat.validate().with_context(|| format!("catalog {}", cat.n))?;
    let path = dir.join(delta_file_name(cat.n));
    ensure!(!dir.join(file_name(cat.n)).exists(), "catalog {} exists already", cat.n);
    let check = |b: &[u8]| -> Result<()> {
        let back = Delta::decode(b)?;
        ensure!(&back.apply(base)? == cat, "delta read back doesn't make the catalog");
        Ok(())
    };
    ensure!(put_once(dir, &path, &d.encode()?, &check)?, "{} appeared meanwhile", path.display());
    Ok(path)
}

/// The sizes of the deltas written since the last whole catalog in `dir` (newest first), and that
/// whole one's size: what `Delta::due_whole` weighs.
pub fn since_whole(dir: &Path) -> Result<(Vec<u64>, Option<u64>)> {
    let names = Names::of_dir(dir)?;
    let mut sizes = Vec::new();
    for n in names.numbers() {
        if names.whole.contains(&n) {
            let len = fs::metadata(dir.join(file_name(n))).with_context(|| format!("catalog {n}"))?.len();
            return Ok((sizes, Some(len)));
        }
        sizes.push(fs::metadata(dir.join(delta_file_name(n))).with_context(|| format!("delta {n}"))?.len());
    }
    Ok((sizes, None))
}

/// How many deltas `dir` holds past its newest whole catalog, from its listing alone (nothing
/// read): about the length of the chain its newest catalog is read through. None when it holds no
/// whole catalog.
pub fn deltas_since_whole(dir: &Path) -> Result<Option<usize>> {
    let names = Names::of_dir(dir)?;
    Ok(names.whole.iter().next_back().map(|w| names.delta.range(w + 1..).count()))
}

/// Whether a delta writer must write a whole catalog now after all: a run of `deltas` (and the one
/// it would add) nearing MAX_CHAIN, where readers stop. The idle-time whole catalog
/// (`whole_due`) comes long before; this is the backstop for when idle time never comes.
pub fn chain_full(deltas: usize) -> bool {
    deltas + 1 >= MAX_CHAIN * 9 / 10
}

/// Whether the catalogs in `dir` are due a whole one (written in idle time: `catalog --whole`):
/// the deltas since the last whole one add up to its size, or their run is half MAX_CHAIN long
/// (`Delta::due_whole`). False with no whole catalog (the next catalog is whole anyway) or no
/// delta since one. Stats each delta since the last whole catalog.
pub fn whole_due(dir: &Path) -> Result<bool> {
    let (since, whole) = since_whole(dir)?;
    Ok(match whole {
        Some(w) if !since.is_empty() => Delta::due_whole(&since, 0, w),
        _ => false,
    })
}

/// The catalogs catalog `n` in `dir` is read through: itself, and back by its deltas' `of` to the
/// whole one they start from (what's kept while `n` is). An error when a delta doesn't read.
pub fn chain(dir: &Path, n: u64) -> Result<Vec<u64>> {
    let names = Names::of_dir(dir)?;
    let mut out = vec![n];
    let mut at = n;
    while !names.whole.contains(&at) && names.delta.contains(&at) && out.len() <= MAX_CHAIN {
        let p = dir.join(delta_file_name(at));
        let d = Delta::decode(&fs::read(&p).with_context(|| format!("read {}", p.display()))?)
            .with_context(|| format!("{}", p.display()))?;
        at = d.of;
        out.push(at);
    }
    Ok(out)
}

/// The number the next publish takes: one past the highest present (damaged ones included, since
/// their names are taken), or 1.
pub fn next_n(dir: &Path) -> Result<u64> {
    Ok(list(dir)?.first().map_or(1, |n| n + 1))
}

/// Deletes all but the `keep` highest catalogs in `dir`. For a Mac's own copies only: on the NAS,
/// deletions go through SSH (plan §3); a Mac keeps its copies whole (`write_copy`), so none is a
/// delta another needs. Returns how many were deleted.
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

    fn sample(n: u64) -> Catalog {
        let mut c = Catalog::new(n);
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
        assert_eq!(Catalog::decode(&c.encode().unwrap()).unwrap(), c);
        assert_eq!(c.encode().unwrap(), c.encode().unwrap(), "deterministic");

        // A newer publisher's extra fields survive a read and a write.
        let mut v = serde_json::to_value(&c).unwrap();
        v["future"] = json!({"x": 1});
        v["files"]["global/pois"]["etag"] = json!("abc");
        v["layers"]["roads"]["tilesize"] = json!(512);
        let c2: Catalog = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(c2.extra["future"], json!({"x": 1}));
        assert_eq!(serde_json::to_value(&c2).unwrap(), v);

        // Missing optional parts are fine; missing fmt or n isn't.
        let minimal: Catalog = serde_json::from_str(r#"{"fmt":1,"n":3}"#).unwrap();
        assert_eq!(minimal.n, 3);
        assert!(serde_json::from_str::<Catalog>(r#"{"fmt":1}"#).is_err());
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

        // Writing the same catalog again is a no-op; different content under a used number is not.
        write(&d, &sample(3)).unwrap();
        let mut other = sample(3);
        other.app = "0.2.0".into();
        assert!(write(&d, &other).is_err());
        assert_eq!(latest(&d).unwrap().unwrap(), sample(3));

        // A truncated newest catalog (a copy cut short) is skipped for the one before.
        let good4 = sample(4).encode().unwrap();
        fs::write(d.join("4.json.zst"), &good4[..good4.len() - 3]).unwrap();
        assert_eq!(latest(&d).unwrap().unwrap().n, 3);
        // So is garbage, and a file whose number disagrees with its content.
        fs::write(d.join("5.json.zst"), b"not zstd").unwrap();
        fs::write(d.join("6.json.zst"), sample(2).encode().unwrap()).unwrap();
        // And a catalog in a format this app doesn't read.
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
        assert!(latest_nas(&pool, &d).unwrap().is_none());
        write(&d, &sample(1)).unwrap();
        write(&d, &sample(2)).unwrap();
        fs::write(d.join("3.json.zst"), b"damaged").unwrap();
        assert_eq!(latest_nas(&pool, &d).unwrap().unwrap().n, 2);
        // Offline is an error, not "no catalog".
        pool.mark_offline("test");
        let e = latest_nas(&pool, &d).unwrap_err();
        assert!(matches!(IoError::find(&e), Some(IoError::Offline)), "{e:#}");
    }

    /// Catalog `n` of a run: each map update some tiles drawn again, a unit added, a region's shapes
    /// redrawn or a region added, now and then one taken away (as map updates change a catalog).
    fn evolve(prev: &Catalog, n: u64) -> Catalog {
        let mut c = prev.clone();
        c.n = n;
        c.created = format!("2026-10-15T{:02}:00:00Z", n % 24);
        let add = |c: &mut Catalog, logical: &str, ext: &str| {
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
            // (A unit the coverage no longer builds: gone from the catalog.)
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
    fn a_catalog_read_through_its_deltas_is_the_whole_one_exactly() {
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
        // Every catalog by number, and the newest, as the whole ones written beside.
        for n in 1..=12 {
            assert_eq!(read_n(&d, n).unwrap(), read_n(&whole, n).unwrap(), "catalog {n}");
        }
        assert_eq!(latest(&d).unwrap().unwrap(), latest(&whole).unwrap().unwrap());
        let pool = IoPool::new(2, Duration::from_secs(60), dir.path().to_owned());
        assert_eq!(latest_nas(&pool, &d).unwrap().unwrap(), prev);
        // From one this Mac has: the deltas after it applied to it; itself when it's the newest.
        let have = read_n(&whole, 10).unwrap();
        assert_eq!(latest_nas_from(&pool, &d, Some(&have)).unwrap().unwrap(), prev);
        assert_eq!(latest_nas_from(&pool, &d, Some(&prev)).unwrap().unwrap(), prev);
        // The chains: back to the whole one each starts from.
        assert_eq!(chain(&d, 11).unwrap(), [11, 10, 9]);
        assert_eq!(chain(&d, 9).unwrap(), [9]);
        let (since, w) = since_whole(&d).unwrap();
        assert_eq!((since.len(), w.is_some()), (0, true));
        let (since, _) = since_whole(&dir.path().join("none")).unwrap();
        assert!(since.is_empty());
        // A delta is smaller than the whole catalog it makes.
        assert!(
            fs::metadata(d.join("11.delta.json.zst")).unwrap().len()
                < fs::metadata(whole.join("11.json.zst")).unwrap().len() / 2
        );
        // An older app's reader lists only the whole ones (its names): it keeps to catalog 12's
        // last whole one, 12 itself here.
        assert!(fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter_map(|e| number(e.file_name().to_str()?))
            .all(|n| n % 3 == 0 || n == 1));
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
        // Applied to another catalog 2 than the one it changes: refused by its digest.
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
    fn whole_again_once_the_deltas_add_up_to_a_whole_one() {
        assert!(!Delta::due_whole(&[], 10, 100));
        assert!(!Delta::due_whole(&[40, 40], 19, 100));
        assert!(Delta::due_whole(&[40, 40], 20, 100));
        assert!(Delta::due_whole(&[], 100, 100));
        // (However small, before the readers' limit.)
        assert!(Delta::due_whole(&vec![1; MAX_CHAIN / 2 - 1], 1, u64::MAX));
        assert!(!Delta::due_whole(&vec![1; MAX_CHAIN / 2 - 2], 1, u64::MAX));
    }

    /// The deltas past the newest whole catalog counted from the listing; a whole one due once
    /// they add up to it (in idle time: `whole_due`); a delta writer's backstop near MAX_CHAIN.
    #[test]
    fn deltas_since_the_whole_one_and_whether_its_due() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("catalog");
        assert_eq!(deltas_since_whole(&d).unwrap(), None);
        assert!(!whole_due(&d).unwrap());
        let one = sample(1);
        write(&d, &one).unwrap();
        assert_eq!(deltas_since_whole(&d).unwrap(), Some(0));
        assert!(!whole_due(&d).unwrap());
        let mut prev = one;
        let whole = fs::metadata(d.join(file_name(1))).unwrap().len();
        let mut sum = 0;
        let mut n = 2;
        while sum < whole {
            assert!(!whole_due(&d).unwrap(), "{sum} of {whole}");
            let next = evolve(&prev, n);
            write_delta(&d, &prev, &next).unwrap();
            sum += fs::metadata(d.join(delta_file_name(n))).unwrap().len();
            prev = next;
            n += 1;
        }
        assert!(whole_due(&d).unwrap());
        assert_eq!(deltas_since_whole(&d).unwrap(), Some(n as usize - 2));
        write(&d, &evolve(&prev, n)).unwrap();
        assert_eq!(deltas_since_whole(&d).unwrap(), Some(0));
        assert!(!whole_due(&d).unwrap());
        assert!(!chain_full(MAX_CHAIN / 2) && chain_full(MAX_CHAIN * 9 / 10 - 1));
    }
}
