//! The 3D buildings' downloaded sources as the agent keys them (docs/buildings3d.md §3.2): which
//! Overture files and row groups, and which GHSL tiles, `bldprep T` reads for a z6 tile T, from the
//! indexes `dem/bldfetch.py` writes beside them on the NAS (`sources/overture/<release>/`
//! `buildings.json` and `footers.json.gz`, `sources/ghsl/R2023A/index.json`). Chosen as
//! `dem/bldprep.py` chooses them: a file listed as downloaded whose footer has the same ETag; its row
//! groups whose box meets T (a parts file's: T grown by `PART_MARGIN_DEG`); the GHSL tiles whose box
//! meets T (none without an index). A listed file whose footer is missing or another object's is
//! skipped where its listed box is far from T, and fails bldprep.py where it meets T: there it's
//! named in T's reads (`stale`), so the tile's key changes once it's fetched again.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How far around T a parts file's row groups are read (degrees: dem/bldprep.py's PART_MARGIN_DEG).
pub const PART_MARGIN_DEG: f64 = 0.02;
/// GHSL's folder (dem/bldprep.py's GHSL_DIR).
pub const GHSL_DIR: &str = "sources/ghsl/R2023A";

/// The release's folder on the NAS (its dot a dash, as `sources/buildings/` has it).
pub fn release_dir(release: &str) -> String {
    format!("sources/overture/{}", release.replace('.', "-"))
}

/// A downloaded Overture file: its name under the release's folder, ETag, whether it's of building
/// parts, and its row groups' boxes (w, s, e, n) and rows.
#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub name: String,
    pub etag: String,
    pub part: bool,
    pub rgs: Vec<([f64; 4], u64)>,
    /// Its row groups' boxes together: what's far from it is skipped at once.
    pub bbox: [f64; 4],
}

/// What `bldprep` can read: the release's downloaded files and the GHSL tiles (name, size, box).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources {
    pub release: String,
    pub files: Vec<File>,
    /// Listed files whose footer is missing or of another ETag (bldprep.py refuses them where they
    /// meet the tile): their names and listed boxes.
    pub stale: Vec<(String, [f64; 4])>,
    pub ghsl: Vec<(String, u64, [f64; 4])>,
}

#[derive(Deserialize)]
struct Listed {
    files: BTreeMap<String, ListedFile>,
}
#[derive(Deserialize)]
struct ListedFile {
    etag: String,
    #[serde(default)]
    bbox: Option<[f64; 4]>,
}
#[derive(Deserialize)]
struct Footer {
    etag: String,
    rgs: Vec<Vec<f64>>,
}
#[derive(Deserialize)]
struct GhslIndex {
    tiles: BTreeMap<String, GhslTile>,
}
#[derive(Deserialize)]
struct GhslTile {
    size: u64,
    bbox: [f64; 4],
}

fn meets(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

impl Sources {
    /// Reads the indexes under `root`: None when the release has no `buildings.json` (nothing
    /// downloaded yet); an error when one is there and can't be read. A listed file whose footer is
    /// missing or of another ETag is `stale` (bldprep.py refuses it where it meets a tile).
    pub fn read(root: &Path, release: &str) -> Result<Option<Sources>> {
        let base = root.join(release_dir(release));
        let listed = match std::fs::read(base.join("buildings.json")) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context("buildings.json"),
        };
        let listed: Listed = serde_json::from_slice(&listed).context("buildings.json")?;
        let gz = std::fs::read(base.join("footers.json.gz")).context("footers.json.gz")?;
        let mut json = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&gz[..]), &mut json).context("footers.json.gz")?;
        let footers: BTreeMap<String, Footer> = serde_json::from_slice(&json).context("footers.json.gz")?;
        let (mut files, mut stale) = (Vec::new(), Vec::new());
        for (name, f) in listed.files {
            let Some(ft) = footers.get(&format!("release/{release}/{name}")).filter(|ft| ft.etag == f.etag) else {
                // (No box listed: taken as meeting every tile, as bldprep.py takes it.)
                stale.push((name, f.bbox.unwrap_or([-180.0, -90.0, 180.0, 90.0])));
                continue;
            };
            let rgs: Vec<([f64; 4], u64)> = ft.rgs.iter().filter(|g| g.len() >= 4).map(|g| ([g[0], g[1], g[2], g[3]], g.get(4).copied().unwrap_or(0.0) as u64)).collect();
            let bbox = rgs.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, (g, _)| [b[0].min(g[0]), b[1].min(g[1]), b[2].max(g[2]), b[3].max(g[3])]);
            files.push(File { part: name.contains("type=building_part/"), name, etag: f.etag, rgs, bbox });
        }
        let ghsl = match std::fs::read(root.join(GHSL_DIR).join("index.json")) {
            Ok(b) => serde_json::from_slice::<GhslIndex>(&b).context("GHSL's index.json")?.tiles.into_iter().map(|(n, t)| (n, t.size, t.bbox)).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).context("GHSL's index.json"),
        };
        Ok(Some(Sources { release: release.to_string(), files, stale, ghsl }))
    }

    /// What `bldprep` reads for z6 tile (x, y), as lines: each file with a row group meeting it, by
    /// name and ETag, with those row groups' indexes; each GHSL tile meeting it, by name and size.
    /// Empty: it reads nothing.
    pub fn read_for(&self, x: u32, y: u32) -> Vec<String> {
        let b = super::tile_box_deg(6, x, y);
        let pb = [b[0] - PART_MARGIN_DEG, b[1] - PART_MARGIN_DEG, b[2] + PART_MARGIN_DEG, b[3] + PART_MARGIN_DEG];
        let mut out = Vec::new();
        for f in &self.files {
            let bx = if f.part { pb } else { b };
            if !meets(f.bbox, bx) {
                continue;
            }
            let hit: Vec<String> = f.rgs.iter().enumerate().filter(|(_, g)| meets(g.0, bx)).map(|(k, _)| k.to_string()).collect();
            if !hit.is_empty() {
                out.push(format!("{} {} {}", f.name, f.etag, hit.join(",")));
            }
        }
        for (n, bb) in &self.stale {
            if meets(*bb, if n.contains("type=building_part/") { pb } else { b }) {
                out.push(format!("stale {n}"));
            }
        }
        for (n, size, bb) in &self.ghsl {
            if meets(*bb, b) {
                out.push(format!("ghsl {n} {size}"));
            }
        }
        out
    }

    /// The rows of the row groups bldprep reads for z6 tile (x, y): what its memory goes by (B1:
    /// 0.3 GB and 160 B a row read; a row group's rows outside T counted too, so a little over).
    pub fn rows_for(&self, x: u32, y: u32) -> u64 {
        let b = super::tile_box_deg(6, x, y);
        let pb = [b[0] - PART_MARGIN_DEG, b[1] - PART_MARGIN_DEG, b[2] + PART_MARGIN_DEG, b[3] + PART_MARGIN_DEG];
        self.files.iter().filter(|f| meets(f.bbox, if f.part { pb } else { b })).flat_map(|f| f.rgs.iter().filter(move |g| meets(g.0, if f.part { pb } else { b })).map(|g| g.1)).sum()
    }
}

/// What `Memo` was read for: the root, the release, and its indexes' sizes and times.
type Stamp = (PathBuf, String, Vec<(u64, u64)>);

/// `Sources::read`, kept while its indexes' sizes and times stay the same (the agent plans every
/// loop; the footers are 2.6 MB of gzip'd JSON).
#[derive(Default)]
pub struct Memo {
    at: Option<Stamp>,
    held: Option<std::sync::Arc<Sources>>,
}

impl Memo {
    pub fn get(&mut self, root: &Path, release: &str) -> Result<Option<std::sync::Arc<Sources>>> {
        let base = root.join(release_dir(release));
        let stamp: Vec<(u64, u64)> = [base.join("buildings.json"), base.join("footers.json.gz"), root.join(GHSL_DIR).join("index.json")]
            .iter()
            .map(|p| std::fs::metadata(p).map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64))).unwrap_or((0, 0)))
            .collect();
        let key = (root.to_path_buf(), release.to_string(), stamp);
        if self.at.as_ref() == Some(&key) {
            return Ok(self.held.clone());
        }
        let s = Sources::read(root, release)?.map(std::sync::Arc::new);
        self.at = Some(key);
        self.held = s.clone();
        Ok(s)
    }
}

/// The sources as the agent's plan reads them (agent::input_digests): "bld-release" (the release
/// when its indexes read, "?" when they can't be read now, "" before anything was downloaded),
/// and for each z6 tile that reads anything, "bldprep 6/x/y" (`Sources::read_for`, hashed) and
/// "bldprep-rows 6/x/y" (`Sources::rows_for`). Worked out again only when an index changes.
pub fn digests(root: &Path, release: &str) -> BTreeMap<String, String> {
    type Kept = (Memo, Option<(std::sync::Arc<Sources>, BTreeMap<String, String>)>);
    static KEPT: std::sync::Mutex<Option<Kept>> = std::sync::Mutex::new(None);
    let mut g = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let (memo, made) = g.get_or_insert_with(|| (Memo::default(), None));
    let s = match memo.get(root, release) {
        Ok(Some(s)) => s,
        Ok(None) => return BTreeMap::from([("bld-release".to_string(), String::new())]),
        Err(e) => {
            eprintln!("bld: the sources' indexes: {e:#}");
            return BTreeMap::from([("bld-release".to_string(), "?".to_string())]);
        }
    };
    if let Some((held, d)) = made.as_ref().filter(|(held, _)| std::sync::Arc::ptr_eq(held, &s)) {
        let _ = held;
        return d.clone();
    }
    let mut d = BTreeMap::from([("bld-release".to_string(), release.to_string())]);
    for x in 0..64u32 {
        for y in 0..64u32 {
            let lines = s.read_for(x, y);
            if lines.is_empty() {
                continue;
            }
            d.insert(format!("bldprep 6/{x}/{y}"), store::naming::hash16(lines.join("\n").as_bytes()));
            d.insert(format!("bldprep-rows 6/{x}/{y}"), s.rows_for(x, y).to_string());
        }
    }
    *made = Some((s, d.clone()));
    d
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A release's indexes under `root`: a buildings file over Tokyo's z6 tile 6/56/25 (two row
    /// groups, one in it), a parts file whose row group lies just outside it (0.01° west), and a
    /// file listed with a stale footer; a GHSL tile over Japan.
    pub(crate) fn write(root: &Path, release: &str) {
        let base = root.join(release_dir(release));
        std::fs::create_dir_all(&base).unwrap();
        let b = crate::bld::tile_box_deg(6, 56, 25);
        let listed = serde_json::json!({"files": {
            "theme=buildings/type=building/a.parquet": {"etag": "ea"},
            "theme=buildings/type=building_part/p.parquet": {"etag": "ep"},
            "theme=buildings/type=building/stale.parquet": {"etag": "new", "bbox": [0.0, 0.0, 1.0, 1.0]},
            "theme=buildings/type=building/stale-near.parquet": {"etag": "new", "bbox": [b[0], b[1], b[0] + 0.5, b[1] + 0.5]},
        }});
        std::fs::write(base.join("buildings.json"), serde_json::to_vec(&listed).unwrap()).unwrap();
        let footers = serde_json::json!({
            format!("release/{release}/theme=buildings/type=building/a.parquet"): {"etag": "ea", "rgs": [[b[0] + 0.1, b[1] + 0.1, b[0] + 0.2, b[1] + 0.2, 1000], [0.0, 0.0, 1.0, 1.0, 5]]},
            format!("release/{release}/theme=buildings/type=building_part/p.parquet"): {"etag": "ep", "rgs": [[b[0] - 0.05, b[1], b[0] - 0.01, b[3], 20]]},
            format!("release/{release}/theme=buildings/type=building/stale.parquet"): {"etag": "old", "rgs": [[b[0], b[1], b[2], b[3], 7]]},
        });
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&serde_json::to_vec(&footers).unwrap()).unwrap();
        std::fs::write(base.join("footers.json.gz"), gz.finish().unwrap()).unwrap();
        std::fs::create_dir_all(root.join(GHSL_DIR)).unwrap();
        let g = serde_json::json!({"tiles": {"G_R5_C30.zip": {"size": 123, "bbox": [130.0, 30.0, 140.0, 40.0]}, "G_R1_C1.zip": {"size": 9, "bbox": [-180.0, 80.0, -170.0, 90.0]}}});
        std::fs::write(root.join(GHSL_DIR).join("index.json"), serde_json::to_vec(&g).unwrap()).unwrap();
    }

    #[test]
    fn what_bldprep_reads_of_a_tile() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Sources::read(d.path(), "2026-09-23.1").unwrap(), None, "nothing downloaded");
        write(d.path(), "2026-09-23.1");
        let s = Sources::read(d.path(), "2026-09-23.1").unwrap().unwrap();
        // (The stale files left out of what's read; the one meeting the tile named in its reads,
        // bldprep.py failing on it there; the far one not.)
        assert_eq!((s.files.len(), s.stale.len()), (2, 2));
        assert_eq!(s.read_for(56, 25), ["theme=buildings/type=building/a.parquet ea 0", "theme=buildings/type=building_part/p.parquet ep 0", "stale theme=buildings/type=building/stale-near.parquet", "ghsl G_R5_C30.zip 123"]);
        assert_eq!(s.rows_for(56, 25), 1020);
        // The tile west of it: the parts' row group only (it meets that tile itself), no building's.
        assert_eq!(s.read_for(55, 25), ["theme=buildings/type=building_part/p.parquet ep 0", "stale theme=buildings/type=building/stale-near.parquet", "ghsl G_R5_C30.zip 123"]);
        assert!(s.read_for(10, 10).is_empty());
        // Kept while the indexes stay as they are; read again once one changes.
        let mut m = Memo::default();
        let a = m.get(d.path(), "2026-09-23.1").unwrap().unwrap();
        assert!(std::sync::Arc::ptr_eq(&a, &m.get(d.path(), "2026-09-23.1").unwrap().unwrap()));
        std::fs::write(d.path().join(GHSL_DIR).join("index.json"), br#"{"tiles": {}}"#).unwrap();
        assert!(m.get(d.path(), "2026-09-23.1").unwrap().unwrap().ghsl.is_empty());
    }
}
