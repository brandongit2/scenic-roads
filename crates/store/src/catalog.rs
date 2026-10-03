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
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The catalog format version written.
pub const FMT: u32 = 1;
/// The oldest version read (plan §8).
pub const MIN_FMT: u32 = 1;

const LEVEL: i32 = 9;
const SUFFIX: &str = ".json.zst";

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
    /// The app version that published it.
    #[serde(default)]
    pub app: String,
    /// Every file the catalog references, by logical name. GC keeps exactly these (plus 14 days of
    /// history).
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
    /// Small worldwide files by what they are (e.g. "pois.json" → logical name).
    #[serde(default)]
    pub global: BTreeMap<String, String>,
    /// The app's meta (bounds, elevation histogram, DEM counts …).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub meta: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub credits: Value,
    /// `{"regions": […], "outline": "<logical of the coverage GeoJSON>"}`.
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

    /// The logical name of the coverage outline, if any.
    pub fn outline(&self) -> Option<&str> {
        self.coverage.get("outline").and_then(Value::as_str)
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
        for (what, m) in [("base", &self.base), ("roads", &self.roads), ("hidata", &self.hidata), ("global", &self.global)] {
            for (k, v) in m {
                out.push((format!("{what}[{k}]"), v.as_str()));
            }
        }
        if let Some(o) = self.outline() {
            out.push(("coverage.outline".into(), o));
        }
        out
    }

    /// Checks that every reference resolves through `files`, and that every file's content name
    /// is well formed and belongs to its logical name. Publishers check before writing.
    pub fn validate(&self) -> Result<()> {
        ensure!(self.fmt == FMT, "catalog format {} (this app writes {FMT})", self.fmt);
        for (logical, f) in &self.files {
            let c = parse_content_name(&f.file).with_context(|| format!("files[{logical}]: {:?} isn't a content name", f.file))?;
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
        ensure!((MIN_FMT..=FMT).contains(&c.fmt), "catalog format {} isn't supported (this app reads {MIN_FMT}–{FMT})", c.fmt);
        Ok(c)
    }
}

/// `<n>.json.zst` → n.
fn number(name: &str) -> Option<u64> {
    let digits = name.strip_suffix(SUFFIX)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The file name of catalog `n`.
pub fn file_name(n: u64) -> String {
    format!("{n}{SUFFIX}")
}

/// Publishes `cat` into `dir` as `<n>.json.zst` after checking it (`Catalog::validate`).
pub fn write(dir: &Path, cat: &Catalog) -> Result<PathBuf> {
    cat.validate().with_context(|| format!("catalog {}", cat.n))?;
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
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let tmp = tmp_path(&path);
    let written = (|| -> Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        let back = Catalog::decode(&fs::read(&tmp)?)?;
        ensure!(&back == cat, "catalog read back differs from what was written");
        Ok(())
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e.context(format!("write {}", tmp.display())));
    }
    match rename_no_replace(&tmp, &path) {
        Ok(()) => Ok(path),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&tmp);
            same_as_existing(&path, cat)?.with_context(|| format!("{} appeared meanwhile with other content", path.display()))
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

/// Reads one catalog file.
pub fn read(path: &Path) -> Result<Catalog> {
    let b = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Catalog::decode(&b).with_context(|| format!("{}", path.display()))
}

/// The catalog numbers in `dir`, highest first (none when `dir` doesn't exist).
pub fn list(dir: &Path) -> Result<Vec<u64>> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
    };
    let mut ns = Vec::new();
    for e in rd {
        let e = e.with_context(|| format!("list {}", dir.display()))?;
        if let Some(n) = e.file_name().to_str().and_then(number) {
            ns.push(n);
        }
    }
    ns.sort_unstable_by(|a, b| b.cmp(a));
    Ok(ns)
}

/// The highest catalog in `dir` that decodes and parses; damaged ones are logged and skipped.
pub fn latest(dir: &Path) -> Result<Option<Catalog>> {
    pick(dir, list(dir)?, |p| match fs::read(p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            eprintln!("catalog: skipping {}: {e}", p.display());
            Ok(None)
        }
    })
}

/// `latest` for a directory on the NAS: every listing and read goes through the I/O pool. The
/// NAS going offline midway is an error, never a reason to fall back to an older catalog.
pub fn latest_nas(pool: &IoPool, dir: &Path) -> Result<Option<Catalog>> {
    let ns = match pool.list(dir) {
        Ok(items) => {
            let mut ns: Vec<u64> = items.iter().filter(|i| !i.is_dir).filter_map(|i| number(&i.name)).collect();
            ns.sort_unstable_by(|a, b| b.cmp(a));
            ns
        }
        Err(IoError::Io(e)) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
    };
    pick(dir, ns, |p| match pool.read_all(p) {
        Ok(b) => Ok(Some(b)),
        Err(IoError::Io(e)) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(IoError::Io(e)) => {
            eprintln!("catalog: skipping {}: {e}", p.display());
            Ok(None)
        }
        Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
    })
}

/// The first of `ns` (highest first) that reads, decodes and carries its own number.
fn pick(dir: &Path, ns: Vec<u64>, mut read: impl FnMut(&Path) -> Result<Option<Vec<u8>>>) -> Result<Option<Catalog>> {
    for n in ns {
        let p = dir.join(file_name(n));
        let Some(bytes) = read(&p)? else { continue };
        match Catalog::decode(&bytes) {
            Ok(c) if c.n == n => return Ok(Some(c)),
            Ok(c) => eprintln!("catalog: skipping {}: it says it is number {}", p.display(), c.n),
            Err(e) => eprintln!("catalog: skipping {}: {e:#}", p.display()),
        }
    }
    Ok(None)
}

/// The number the next publish takes: one past the highest present (damaged ones included, since
/// their names are taken), or 1.
pub fn next_n(dir: &Path) -> Result<u64> {
    Ok(list(dir)?.first().map_or(1, |n| n + 1))
}

/// Deletes all but the `keep` highest catalogs in `dir`. For a Mac's own copies only: on the NAS,
/// deletions go through SSH (plan §3). Returns how many were deleted.
pub fn prune(dir: &Path, keep: usize) -> Result<usize> {
    let mut gone = 0;
    for n in list(dir)?.into_iter().skip(keep) {
        let p = dir.join(file_name(n));
        match fs::remove_file(&p) {
            Ok(()) => gone += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("remove {}", p.display())),
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
            c.files.insert(logical.into(), FileRef { file: content_name(logical, &h, ext), size, fmt: 1, extra: BTreeMap::new() });
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
        let outline = add("global/coverage", "geojson", 7);
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
        c.credits = json!(["OpenStreetMap contributors"]);
        c.coverage = json!({"regions": ["northumberland"], "outline": outline});
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
        let pool = IoPool::new(2, Duration::from_secs(5), dir.path().to_owned());
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
}
