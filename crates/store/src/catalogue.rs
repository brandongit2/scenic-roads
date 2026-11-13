//! The map as a catalogue (docs/formats.md, Catalogue): every served file by logical name, with its
//! content name, size and format version, and the sections that name them. No catalogue is a file
//! of its own: the map's log on the lead is the map, and
//! `crate::map::Map::catalogue` makes this value of it for the readers (`n` the map's `seq`). Its
//! encoding is the step's map posted to the lead's agent and the recorded test chain's.

use crate::naming::parse_content_name;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;

mod coverage;
pub use coverage::Coverage;

/// The stored and wire name of the step that publishes the map (`catalog`), its history event and
/// the JSON keys naming it. It keeps its old spelling (task #547: stored in the history and job logs,
/// and the pool runs mixed app versions): only the Rust names around it say "catalogue".
pub const NAME: &str = "catalog";
/// The step's job while publishing is held for review (the pool's `hold-catalog`): its map goes to
/// the lead's held maps (`--held`, agentcore's agent::publish::HELD), not the log.
pub const HELD_NAME: &str = "catalog-held";

/// The catalogue format version written. (2, task #449: `markdata` may name one bundle of every
/// tile, docs/formats.md "Bundles", which an app reading only 1 can't read.)
pub const FMT: u32 = 2;
/// The oldest version read (docs/building.md §2).
pub const MIN_FMT: u32 = 1;

const LEVEL: i32 = 9;

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
    /// (inputs::rules::Credit). Empty in catalogues made before they were recorded. Kept as JSON,
    /// as `coverage` is, so a part the app can't read never makes the catalogue unreadable.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub credits: Value,
    /// The elevation datasets' labels by source byte (`src.u8`), from their declarations
    /// (inputs::inputs::credits::dem_labels): how the map names a road's elevation sources. None
    /// in catalogues made before they were recorded.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dem_labels: BTreeMap<u8, String>,
    /// The coverage it was built for: `{"regions": [{"id", "name", "outline", "shapes"}]}`, each
    /// outline entry's polygons simplified for drawing (buildstore::coverage::DrawnRegion), kept as
    /// JSON text (`Coverage`). No regions in catalogues made before they were recorded.
    #[serde(default, skip_serializing_if = "Coverage::is_null")]
    pub coverage: Coverage,
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
    /// worldwide ones (terrain::terrain_deep).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deep: BTreeMap<String, String>,
    /// The native zoom of each deep pack's z10 tile, past the base's (terrain::terrain_deep::
    /// native_zoom; elsewhere the base's finest, z12 to 67°): the map's zoom by area.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub native_zoom: BTreeMap<String, u8>,
    /// A polar layer's block packs (z4–9) by z4 tile of its pole's grid ("4/x/y": terrain::polar,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::content_name;
    use serde_json::json;

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
            "shapes": {"osm:88066": [[[[-2.68941, 55.80271], [-1.46055, 55.07428], [-2.56493, 54.78346], [-2.68941, 55.80271]]]]}}]})
        .into();
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
}
