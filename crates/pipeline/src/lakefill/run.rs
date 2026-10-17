//! The fill made (crate::lakefill; docs/plan.md §6 Water, "Lakes filled from Canada's own data"):
//! the survey of the sheets OSM's small lakes are missing from (`survey`, `gaps`), the fill of
//! those from the source's shapefiles (`fill`, by longitude bands, `merge`), and the whole of it as
//! the `lakefill` step runs it (`all`). The `lakefill` tool runs each part on its own.

use super::nts;
use anyhow::{Context, Result};
use crate::watercov::Raster;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The Canadian regions' outlines (inputs/regions/*.toml): (relation, region).
pub const PROVINCES: [(u64, &str); 13] = [
    (391186, "alberta"),
    (390867, "british-columbia"),
    (390841, "manitoba"),
    (68942, "new-brunswick"),
    (391196, "newfoundland-and-labrador"),
    (391220, "northwest-territories"),
    (390558, "nova-scotia"),
    (390840, "nunavut"),
    (68841, "ontario"),
    (391115, "prince-edward-island"),
    (61549, "quebec"),
    (391178, "saskatchewan"),
    (391455, "yukon"),
];


/// The survey's tile zoom and the pixels a side it draws each tile's water at.
const SZ: u8 = 12;
const PX: usize = 64;

/// A sheet as the survey found it (`survey`), a row of its CSV (`sheets_csv`).
#[derive(Clone, Debug, PartialEq)]
pub struct SheetRow {
    pub sheet: String,
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
    pub region: String,
    pub land_km2: f64,
    pub sea_km2: f64,
    pub inland_km2: f64,
    /// Lakes and ponds by ring area: < 0.01, 0.01–0.1, 0.1–1, ≥ 1 km².
    pub lakes: [u32; 4],
    pub z12_bytes: u64,
    pub z12_tiles: u32,
}

impl SheetRow {
    /// The sheet's key (nts).
    pub fn key(&self) -> nts::Key {
        nts::sheet_key(self.west + 1e-6, self.south + 1e-6)
    }
}

const SHEETS_HEAD: &str = "sheet,west,south,east,north,region,land_km2,sea_km2,inland_km2,lakes_lt001,lakes_001_01,lakes_01_1,lakes_ge1,z12_bytes,z12_tiles";

/// The survey's rows as CSV.
pub fn sheets_csv(rows: &[SheetRow]) -> String {
    let mut s = format!("{SHEETS_HEAD}\n");
    for r in rows {
        s.push_str(&format!(
            "{},{},{},{},{},{},{:.3},{:.3},{:.3},{},{},{},{},{},{}\n",
            r.sheet, r.west, r.south, r.east, r.north, r.region, r.land_km2, r.sea_km2, r.inland_km2, r.lakes[0], r.lakes[1], r.lakes[2], r.lakes[3], r.z12_bytes, r.z12_tiles
        ));
    }
    s
}

/// The survey's CSV, read back.
pub fn read_sheets_csv(text: &str) -> Result<Vec<SheetRow>> {
    let mut lines = text.lines();
    anyhow::ensure!(lines.next() == Some(SHEETS_HEAD), "not the survey's sheets");
    lines
        .map(|l| -> Result<SheetRow> {
            let v: Vec<&str> = l.split(',').collect();
            anyhow::ensure!(v.len() == 15, "a row of {} fields: {l}", v.len());
            let f = |i: usize| -> Result<f64> { Ok(v[i].parse()?) };
            let u = |i: usize| -> Result<u64> { Ok(v[i].parse()?) };
            Ok(SheetRow {
                sheet: v[0].into(),
                west: f(1)?,
                south: f(2)?,
                east: f(3)?,
                north: f(4)?,
                region: v[5].into(),
                land_km2: f(6)?,
                sea_km2: f(7)?,
                inland_km2: f(8)?,
                lakes: [u(9)? as u32, u(10)? as u32, u(11)? as u32, u(12)? as u32],
                z12_bytes: u(13)?,
                z12_tiles: u(14)? as u32,
            })
        })
        .collect()
}

/// A gap sheet (`gaps`): its row, its area and land (km²), its small lakes (under 1 km²) a 100 km²
/// of land, its neighbours' reference (their 75th percentile, when it has enough of them), and why
/// it's a gap.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    pub row: SheetRow,
    pub area_km2: f64,
    pub density: f64,
    pub reference: Option<f64>,
    pub why: Vec<&'static str>,
}

/// The neighbourhood `gaps` compares a sheet with (sheets each way), the share of its reference
/// under which it's a gap, the least reference that counts, and the density under which a sheet is
/// a gap whatever its neighbours (small lakes a 100 km² of land).
const RADIUS: i32 = 3;
const SHARE: f64 = 0.1;
const MIN_REF: f64 = 5.0;
const EMPTY: f64 = 2.0;
/// A sheet's land (km²) below which it isn't judged (mostly sea or a big lake).
const MIN_LAND: f64 = 50.0;

/// The sheets whose small lakes OSM lacks, from the survey (`survey`), in the rows' order. A sheet
/// with more than `MIN_LAND` of land is a gap when any of:
/// - **neighbours:** its small lakes a 100 km² of land are under `SHARE` of its neighbours' 75th
///   percentile (within `RADIUS` sheets, at least 8 of them, the gaps found so far left out; the
///   reference at least `MIN_REF`), repeated until no more are found, so it reaches inside blocks;
/// - **empty:** under `EMPTY`;
/// - **truncated:** its size curve cut off: 3 or more lakes over 0.1 km², fewer under.
pub fn gaps(rows: &[SheetRow]) -> Vec<Gap> {
    const R: f64 = 6371.0088;
    struct S {
        width: f64,
        area: f64,
        d: Option<f64>,
        reference: Option<f64>,
        why: Vec<&'static str>,
    }
    let key = |r: &SheetRow| ((r.west / (r.east - r.west)).round() as i64, (r.south * 4.0).round() as i64);
    let mut idx: HashMap<(i64, i64), usize> = HashMap::new();
    let mut ss: Vec<S> = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        let area = R * R * (r.east - r.west).to_radians() * (r.north.to_radians().sin() - r.south.to_radians().sin());
        let land = (area - r.sea_km2 - r.inland_km2).max(0.0);
        let small = f64::from(r.lakes[0] + r.lakes[1] + r.lakes[2]);
        ss.push(S { width: r.east - r.west, area, d: (land > MIN_LAND).then(|| small / land * 100.0), reference: None, why: Vec::new() });
        idx.insert(key(r), i);
    }
    // A sheet's neighbours within `rad` (rows of other widths matched by longitude).
    let neigh = |i: usize| -> Vec<usize> {
        let (c, row) = key(&rows[i]);
        let w = ss[i].width;
        let lon = (c as f64 + 0.5) * w;
        let mut out = Vec::new();
        for dr in -RADIUS..=RADIUS {
            let r2 = row + i64::from(dr);
            for dc in -RADIUS..=RADIUS {
                if dr == 0 && dc == 0 {
                    continue;
                }
                let at = lon + f64::from(dc) * w;
                for wd in [0.5, 1.0, 2.0] {
                    let kk = ((at / wd).floor() as i64, r2);
                    if let Some(&j) = idx.get(&kk) {
                        if ((kk.0 as f64 + 0.5) * ss[j].width - at).abs() < ss[j].width / 2.0 + 1e-9 {
                            out.push(j);
                            break;
                        }
                    }
                }
            }
        }
        out
    };
    let near: Vec<Vec<usize>> = (0..rows.len()).map(neigh).collect();
    let mut gap: HashSet<usize> = HashSet::new();
    for _ in 0..30 {
        let mut new: HashSet<usize> = HashSet::new();
        for i in 0..rows.len() {
            let Some(d) = ss[i].d else { continue };
            let mut ds: Vec<f64> = near[i].iter().filter(|j| !gap.contains(j)).filter_map(|&j| ss[j].d).collect();
            if ds.len() < 8 {
                continue;
            }
            ds.sort_by(f64::total_cmp);
            let reference = ds[(ds.len() as f64 * 0.75) as usize];
            ss[i].reference = Some(reference);
            if reference >= MIN_REF && d < SHARE * reference {
                new.insert(i);
            }
        }
        for (i, r) in rows.iter().enumerate() {
            let Some(d) = ss[i].d else { continue };
            let (tiny, big) = (r.lakes[0] + r.lakes[1], r.lakes[2] + r.lakes[3]);
            let mut why = Vec::new();
            if new.contains(&i) {
                why.push("neighbours");
            }
            if d < EMPTY {
                new.insert(i);
                why.push("empty");
            }
            if big >= 3 && tiny < big {
                new.insert(i);
                why.push("truncated");
            }
            ss[i].why = why;
        }
        if new == gap {
            break;
        }
        gap = new;
    }
    rows.iter()
        .enumerate()
        .filter(|(i, _)| gap.contains(i))
        .map(|(i, r)| Gap { row: r.clone(), area_km2: ss[i].area, density: ss[i].d.unwrap_or(0.0), reference: ss[i].reference, why: ss[i].why.clone() })
        .collect()
}

/// The gap sheets as CSV, by region then sheet.
pub fn gaps_csv(gaps: &[Gap]) -> String {
    let mut g: Vec<&Gap> = gaps.iter().collect();
    g.sort_by(|a, b| (&a.row.region, &a.row.sheet).cmp(&(&b.row.region, &b.row.sheet)));
    let mut s = String::from("sheet,region,west,south,east,north,area_km2,small_lakes_per_100km2,neighbour_ref,why\n");
    for v in g {
        let r = &v.row;
        s.push_str(&format!("{},{},{},{},{},{},{:.0},{:.2},{:.1},{}\n", r.sheet, r.region, r.west, r.south, r.east, r.north, v.area_km2, v.density, v.reference.unwrap_or(0.0), v.why.join("+")));
    }
    s
}

/// The gap sheets of a gap list's CSV (`gaps_csv`, or any CSV with `west` and `south` columns).
pub fn read_gaps_csv(text: &str) -> Result<HashSet<nts::Key>> {
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().context("an empty sheet list")?.split(',').collect();
    let (cw, cs) = (head.iter().position(|h| *h == "west").context("no west")?, head.iter().position(|h| *h == "south").context("no south")?);
    let mut out = HashSet::new();
    for l in lines {
        let v: Vec<&str> = l.split(',').collect();
        let (w, s): (f64, f64) = (v[cw].parse()?, v[cs].parse()?);
        out.insert(nts::sheet_key(w + 1e-6, s + 1e-6));
    }
    Ok(out)
}

/// How the fill reads its source (`fill`): the basemap (OSM's water), the source's shapefiles (folders
/// searched, or zips), the layer and its records kept (`field` one of `keep`), those water only some
/// of the time (`intermittent`: field, value), the rules (crate::lakefill::select), and `confirm`: a
/// sheet is filled only where under that share of the source's lakes in it touch OSM's water (a
/// sheet OSM has its lakes in isn't a gap).
#[derive(Clone, Debug)]
pub struct FillOpts {
    pub basemap: PathBuf,
    pub sources: Vec<PathBuf>,
    pub field: String,
    pub keep: Vec<String>,
    pub layer: String,
    pub intermittent: Option<(String, String)>,
    pub rules: super::Rules,
    pub confirm: Option<f64>,
}

impl FillOpts {
    /// The declared lake-fill source's (crate::inputs::sources, the `lake-fill` role: CanVec
    /// 1:50,000 Hydro, the provinces' shapefile zips): its layer, the field and its values kept
    /// (`params.layer`, `field`, `keep`), the field and value of those water only some of the time
    /// (`params.intermittent`: drawn as water, not flattened), and the share a sheet is confirmed at
    /// (`params.confirm`); its zips under `sources`.
    pub fn declared(basemap: &Path, sources: &Path, s: &crate::inputs::sources::Source) -> Result<FillOpts> {
        let intermittent = match s.data.params.get("intermittent") {
            None => None,
            Some(_) => {
                let v = s.param_strs("intermittent")?;
                anyhow::ensure!(v.len() == 2, "{}: `params.intermittent` is a field and its value", s.id);
                Some((v[0].clone(), v[1].clone()))
            }
        };
        let confirm = match s.data.params.get("confirm") {
            None => None,
            Some(c) => Some(c.as_f64().with_context(|| format!("{}: `params.confirm` isn't a number", s.id))?),
        };
        Ok(FillOpts {
            basemap: basemap.to_path_buf(),
            sources: vec![sources.to_path_buf()],
            field: s.param_str("field")?.into(),
            keep: s.param_strs("keep")?,
            layer: s.param_str("layer")?.into(),
            intermittent,
            rules: super::Rules::default(),
            confirm,
        })
    }
}

/// The lake fill's source as its key names it, under the project root `root` and records `m`: its
/// zips' pin (`source_pin` of `source_dir`), and the declaration (`Source::key_extra`: nothing while
/// it names what the code read, agent::rekey::declared_before::LAKE_FILL). Ok(None): its folder holds
/// no zip (no fill made); Err: not declared, its declared folder not there (the zips not moved yet,
/// or the NAS away), or not to be read now: the step and what reads its fill wait.
pub fn planned_source(root: &Path, m: &std::collections::BTreeMap<String, String>) -> std::result::Result<Option<String>, String> {
    let s = crate::inputs::sources::open(root, m).map_err(|e| format!("the declared sources can't be read now ({e:#})"))?.ok_or("the source declarations have no accepted version yet")?;
    let src = s.role(crate::inputs::sources::LAKE_FILL).map_err(|e| format!("{e:#}"))?;
    let dir = root.join(source_dir(src));
    if !dir.is_dir() {
        return Err(format!("its source's folder, {}, isn't there (its zips are kept there: docs/inputs.md §6.9)", source_dir(src)));
    }
    let pin = source_pin(&dir).map_err(|e| format!("its source's folder can't be read now ({e})"))?;
    Ok(pin.map(|p| match src.key_extra(crate::agent::rekey::declared_before::LAKE_FILL) {
        Some(x) => format!("{p}\n{x}"),
        None => p,
    }))
}

/// Where the declared source's zips are kept under the project root: `sources/fetched/<id>/` (by
/// hand: they're downloaded from its `url`, docs/inputs.md §6.9).
pub fn source_dir(s: &crate::inputs::sources::Source) -> String {
    format!("sources/fetched/{}", s.id)
}

/// The source's files under `dir` as the step's key names them: each zip's name, size and time
/// (a zip replaced by one of the same size is a new source), in order; None when there's no such
/// folder or no zip in it (no fill made); an error when it can't be read now (the NAS away), which
/// is never taken for "no source".
pub fn source_pin(dir: &Path) -> std::io::Result<Option<String>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut v = Vec::new();
    for e in rd {
        let e = e?;
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.to_ascii_lowercase().ends_with(".zip") || n.starts_with('.') {
            continue;
        }
        let md = e.metadata()?;
        let t = md.modified()?.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        v.push(format!("{n} {} {t}", md.len()));
    }
    v.sort();
    Ok((!v.is_empty()).then(|| v.join("\n")))
}

/// The longitude bands `all` fills one at a time (each 1.8–7.2 GB of memory, 2026-10-09).
pub const BANDS: [(f64, f64); 6] = [(-142.0, -125.0), (-125.0, -110.0), (-110.0, -100.0), (-100.0, -85.0), (-85.0, -70.0), (-70.0, -50.0)];

/// The whole fill, as the `lakefill` step makes it: the survey of the basemap `basemap` over the
/// outlines `outlines`, its gap sheets, and the fill of them from the declared lake-fill source
/// `decl`'s zips under `sources` (`FillOpts::declared`) band by band (`BANDS`), merged; in `work` (a
/// local folder: the survey, each band's run and the merged fill with its report). `part` is told each part as it starts (0 the
/// survey, then each band, then the merge); `said`, the survey's progress. Returns the fill's file.
pub fn all(basemap: &Path, outlines: &Path, sources: &Path, decl: &crate::inputs::sources::Source, work: &Path, part: &dyn Fn(usize), said: &(dyn Fn(u64, u64) + Sync)) -> Result<PathBuf> {
    std::fs::create_dir_all(work)?;
    part(0);
    let rows = survey(basemap, outlines, said)?;
    std::fs::write(work.join("sheets.csv"), sheets_csv(&rows))?;
    let g = gaps(&rows);
    std::fs::write(work.join("gap-sheets.csv"), gaps_csv(&g))?;
    let keys: HashSet<nts::Key> = g.iter().map(|v| v.row.key()).collect();
    eprintln!("lakefill: {} of {} sheets are gaps", keys.len(), rows.len());
    let o = FillOpts::declared(basemap, sources, decl)?;
    let mut runs = Vec::new();
    for (k, &b) in BANDS.iter().enumerate() {
        part(1 + k);
        let d = work.join(format!("band-{k}"));
        fill(&o, &keys, Some(b), &d)?;
        runs.push(d);
    }
    part(1 + BANDS.len());
    let (f, _) = merge(&runs, &work.join("fill"))?;
    Ok(f)
}

/// A sheet's sums as the survey goes.
#[derive(Default, Clone)]
struct Sheet {
    land_km2: f64,
    sea_km2: f64,
    inland_km2: f64,
    /// Lakes and ponds by ring area: < 0.01, 0.01–0.1, 0.1–1, ≥ 1 km².
    lakes: [u32; 4],
    bytes: u64,
    tiles: u32,
}

fn lonlat(z: u8, x: f64, y: f64) -> (f64, f64) {
    let n = f64::from(1u32 << z);
    let lon = x / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    (lon, lat)
}

/// The survey: every NTS 1:50,000 sheet across the Canadian regions (`PROVINCES`, from the pass's
/// outlines), with the basemap's z12 water in it: its land and water (km², to the metre² as the
/// survey's CSV holds them) and its lakes by size (counted by where their rings' vertices average).
/// In the sheets' order (west to east within a row, rows south to north).
pub fn survey(base: &Path, outlines: &Path, said: &(dyn Fn(u64, u64) + Sync)) -> Result<Vec<SheetRow>> {
    // The regions' outlines (simplified), to say which sheets are Canada's.
    let o = crate::outlines::Outlines::open(outlines)?;
    let mut polys: Vec<(&str, Vec<Vec<[i32; 2]>>)> = Vec::new();
    for (id, name) in PROVINCES {
        let rec = o.by_id(id).with_context(|| format!("outline {id}"))?;
        polys.push((name, o.simple_rings(rec)?));
    }
    let region_at = |lon: f64, lat: f64| -> Option<&str> {
        let p = [(lon * 1e7) as i32, (lat * 1e7) as i32];
        polys.iter().find(|(_, r)| crate::outlines::inside(&r.iter().map(Vec::as_slice).collect::<Vec<_>>(), p)).map(|(n, _)| *n)
    };
    let pm = store::pmtiles::PmTiles::with_leaf_cache(Box::new(store::range::PlainFile::open(base)?), 4096)?;
    // The z5 tiles over Canada's box.
    let (w, east, s, n) = (-141.5, -52.0, 41.5, 83.5);
    let tx = |lon: f64, z: u8| ((lon + 180.0) / 360.0 * f64::from(1u32 << z)).floor() as u32;
    let ty = |lat: f64, z: u8| {
        let r = lat.to_radians();
        ((1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * f64::from(1u32 << z)).floor() as u32
    };
    let fives: Vec<(u32, u32)> = (ty(n, 5)..=ty(s, 5)).flat_map(|y| (tx(w, 5)..=tx(east, 5)).map(move |x| (x, y))).collect();
    let sheets: Mutex<HashMap<(i32, i32), Sheet>> = Mutex::new(HashMap::new());
    let done = std::sync::atomic::AtomicUsize::new(0);
    let base_id = |z: u8| ((1u64 << (2 * u32::from(z))) - 1) / 3;
    fives.par_iter().try_for_each(|&(x5, y5)| -> Result<()> {
        let h5 = store::pmtiles::zxy_to_tile_id(5, x5, y5)? - base_id(5);
        let d = 2 * u32::from(SZ - 5);
        let lo = base_id(SZ) + (h5 << d);
        let mut entries = Vec::new();
        pm.entries_in(lo, lo + (1u64 << d), &mut |e| entries.push(e))?;
        let data = pm.header().data_offset;
        let mut offs: Vec<(u64, u32)> = entries.iter().map(|e| (data + e.offset, e.length)).collect();
        offs.sort_unstable();
        offs.dedup();
        let mut spans: Vec<(u64, u64)> = Vec::new();
        for (o, l) in offs {
            match spans.last_mut() {
                Some(s) if o <= s.1 + (1 << 20) && o + u64::from(l) - s.0 <= 256 << 20 => s.1 = s.1.max(o + u64::from(l)),
                _ => spans.push((o, o + u64::from(l))),
            }
        }
        let mut bufs: Vec<(u64, Vec<u8>)> = Vec::new();
        for (a, b) in spans {
            bufs.push((a, pm.source().read_at(a, usize::try_from(b - a)?)?));
        }
        let bytes_at = |o: u64, l: u32| -> &[u8] {
            let k = bufs.partition_point(|b| b.0 <= o) - 1;
            let (a, ref b) = bufs[k];
            &b[(o - a) as usize..(o - a) as usize + l as usize]
        };
        let mut local: HashMap<(i32, i32), Sheet> = HashMap::new();
        for e in &entries {
            for id in e.tile_id..e.tile_id + u64::from(e.run_length) {
                let (z, x, y) = store::pmtiles::tile_id_to_zxy(id)?;
                debug_assert_eq!(z, SZ);
                let (clon, clat) = lonlat(SZ, f64::from(x) + 0.5, f64::from(y) + 0.5);
                if !(w..=east).contains(&clon) || !(s..=n).contains(&clat) {
                    continue;
                }
                let raw = bytes_at(data + e.offset, e.length);
                let sheet_of = |lon: f64, lat: f64| nts::sheet_key(lon, lat);
                local.entry(sheet_of(clon, clat)).or_default().bytes += u64::from(e.length);
                local.entry(sheet_of(clon, clat)).or_default().tiles += 1;
                let mvt = names::mvt::gunzip_if_gzip(raw)?;
                let tile = names::mvt::Tile::decode(&mvt)?;
                let (mut sea, mut inland) = (Raster::new(PX, PX), Raster::new(PX, PX));
                if let Some(l) = tile.layers.iter().find(|l| l.name == "water") {
                    let k = PX as f64 / f64::from(l.extent);
                    let class_key = l.keys.iter().position(|k| k == "class");
                    for f in &l.features {
                        if f.geom_type != Some(3) {
                            continue;
                        }
                        let class = class_key
                            .and_then(|ck| f.tags.as_chunks::<2>().0.iter().find(|kv| kv[0] as usize == ck))
                            .and_then(|kv| l.values.get(kv[1] as usize))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let rings = crate::lakefill::mvt_rings(&f.geometry);
                        let ras = if class == "ocean" { &mut sea } else { &mut inland };
                        for r in &rings {
                            let mut prev = r[r.len() - 1];
                            for &p in r {
                                ras.edge([prev[0] * k, prev[1] * k], [p[0] * k, p[1] * k]);
                                prev = p;
                            }
                        }
                        if class != "lake" && class != "pond" {
                            continue;
                        }
                        for r in &rings {
                            let a2 = crate::watercov::area2(r);
                            if a2 <= 0.0 {
                                continue;
                            }
                            let (mut sx, mut sy) = (0.0, 0.0);
                            for p in r {
                                sx += p[0];
                                sy += p[1];
                            }
                            let ext = f64::from(l.extent);
                            let (cx, cy) = (sx / r.len() as f64 / ext, sy / r.len() as f64 / ext);
                            if !(0.0..1.0).contains(&cx) || !(0.0..1.0).contains(&cy) {
                                continue;
                            }
                            let (lon, lat) = lonlat(SZ, f64::from(x) + cx, f64::from(y) + cy);
                            // Ring area in km²: tile units² → metres² at this latitude.
                            let m = 40_075_016.686 * lat.to_radians().cos() / f64::from(1u32 << SZ) / ext;
                            let km2 = a2 / 2.0 * m * m / 1e6;
                            let b = if km2 < 0.01 { 0 } else if km2 < 0.1 { 1 } else if km2 < 1.0 { 2 } else { 3 };
                            local.entry(sheet_of(lon, lat)).or_default().lakes[b] += 1;
                        }
                    }
                }
                let (sc, ic) = (sea.coverage(), inland.coverage());
                for py in 0..PX {
                    let (_, lat) = lonlat(SZ, f64::from(x) + 0.5 / PX as f64, f64::from(y) + (py as f64 + 0.5) / PX as f64);
                    let m = 40_075_016.686 * lat.to_radians().cos() / f64::from(1u32 << SZ) / PX as f64;
                    let a = m * m / 1e6;
                    for px in 0..PX {
                        let (lon, _) = lonlat(SZ, f64::from(x) + (px as f64 + 0.5) / PX as f64, 0.0);
                        let s_ = sc[py * PX + px].min(1.0);
                        let i_ = ic[py * PX + px].min(1.0 - s_);
                        let sh = local.entry(sheet_of(lon, lat)).or_default();
                        sh.sea_km2 += f64::from(s_) * a;
                        sh.inland_km2 += f64::from(i_) * a;
                        sh.land_km2 += f64::from(1.0 - s_ - i_) * a;
                    }
                }
            }
        }
        let mut all = sheets.lock().unwrap();
        for (k, v) in local {
            let s = all.entry(k).or_default();
            s.land_km2 += v.land_km2;
            s.sea_km2 += v.sea_km2;
            s.inland_km2 += v.inland_km2;
            for i in 0..4 {
                s.lakes[i] += v.lakes[i];
            }
            s.bytes += v.bytes;
            s.tiles += v.tiles;
        }
        drop(all);
        let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        said(d as u64, fives.len() as u64);
        Ok(())
    })?;
    let sheets = sheets.into_inner().unwrap();
    let mut keys: Vec<_> = sheets.keys().copied().collect();
    keys.sort_unstable();
    let mut rows = Vec::new();
    for k in keys {
        let (wl, sl, el, nl) = nts::bounds(k);
        // Canada's when its centre or most of a 5 × 5 grid of points over it is in a region.
        let mut hits: HashMap<&str, u32> = HashMap::new();
        for i in 0..5 {
            for j in 0..5 {
                let (lon, lat) = (wl + (el - wl) * (f64::from(i) + 0.5) / 5.0, sl + (nl - sl) * (f64::from(j) + 0.5) / 5.0);
                if let Some(r) = region_at(lon, lat) {
                    *hits.entry(r).or_default() += 1;
                }
            }
        }
        let Some((region, _)) = hits.iter().max_by_key(|h| (*h.1, *h.0)) else { continue };
        let s = &sheets[&k];
        // (As the CSV has them: the classification reads the same values from either.)
        let mm = |v: f64| -> f64 { format!("{v:.3}").parse().unwrap_or(v) };
        rows.push(SheetRow {
            sheet: nts::name(k),
            west: wl,
            south: sl,
            east: el,
            north: nl,
            region: region.to_string(),
            land_km2: mm(s.land_km2),
            sea_km2: mm(s.sea_km2),
            inland_km2: mm(s.inland_km2),
            lakes: s.lakes,
            z12_bytes: s.bytes,
            z12_tiles: s.tiles,
        });
    }
    Ok(rows)
}

/// Where a file of the source is: on disk, or a member of a zip.
#[derive(Clone, Debug)]
pub enum Src {
    File(std::path::PathBuf),
    Zip(std::path::PathBuf, crate::lakefill::zip::Member),
}

impl Src {
    pub fn name(&self) -> String {
        match self {
            Src::File(p) => p.display().to_string(),
            Src::Zip(z, m) => format!("{}:{}", z.display(), m.name),
        }
    }

    /// Its bytes, streamed (a zip's member in long reads, its CRC checked at its end).
    pub fn open(&self) -> Result<Box<dyn std::io::Read + Send>> {
        Ok(match self {
            Src::File(p) => Box::new(std::io::BufReader::with_capacity(4 << 20, std::fs::File::open(p)?)),
            Src::Zip(z, m) => crate::lakefill::zip::open(z, m)?,
        })
    }

    pub fn read_to_string(&self) -> Result<String> {
        let mut s = String::new();
        self.open()?.read_to_string(&mut s)?;
        Ok(s)
    }
}

/// The source's shapefiles of `layer` under `src` (a folder, searched, or a zip, its members
/// listed): (the .shp, its .dbf, its .prj).
pub fn shapefiles(src: &Path, layer: &str) -> Result<Vec<(Src, Src, Src)>> {
    let mut out = Vec::new();
    let is_layer = |name: &str| {
        let n = name.to_ascii_lowercase();
        let stem = n.rsplit('/').next().unwrap_or(&n).to_string();
        let l = layer.to_ascii_lowercase();
        // `waterbody_2.shp`, or a part of it (`waterbody_2_8.shp`).
        stem.strip_suffix(".shp").is_some_and(|s| s == l || s.strip_prefix(&format!("{l}_")).is_some_and(|k| k.chars().all(|c| c.is_ascii_digit())))
    };
    if src.is_dir() {
        let mut stack = vec![src.to_path_buf()];
        while let Some(d) = stack.pop() {
            let mut es: Vec<_> = std::fs::read_dir(&d)?.collect::<std::io::Result<_>>()?;
            es.sort_by_key(|e| e.file_name());
            for e in es {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
                    out.extend(shapefiles(&p, layer)?);
                } else if is_layer(&p.to_string_lossy()) {
                    out.push((Src::File(p.clone()), Src::File(p.with_extension("dbf")), Src::File(p.with_extension("prj"))));
                }
            }
        }
    } else {
        let ms = crate::lakefill::zip::members(src)?;
        for m in ms.iter().filter(|m| is_layer(&m.name)) {
            let stem = &m.name[..m.name.len() - 4];
            let find = |ext: &str| ms.iter().find(|n| n.name.eq_ignore_ascii_case(&format!("{stem}.{ext}"))).cloned();
            let dbf = find("dbf").with_context(|| format!("{}: no .dbf", m.name))?;
            let prj = find("prj").with_context(|| format!("{}: no .prj", m.name))?;
            out.push((Src::Zip(src.to_path_buf(), m.clone()), Src::Zip(src.to_path_buf(), dbf), Src::Zip(src.to_path_buf(), prj)));
        }
    }
    Ok(out)
}

/// Whether a box of longitude and latitude comes within `m` degrees of a gap sheet.
pub fn near_gaps(gaps: &HashSet<nts::Key>, b: [f64; 4], m: f64) -> bool {
    let (w, s, e, n) = (b[0] - m, b[1] - m, b[2] + m, b[3] + m);
    let r0 = (s * 4.0).floor() as i32;
    let r1 = (n * 4.0).floor() as i32;
    for row in r0..=r1 {
        let wd = nts::width(row);
        let (c0, c1) = ((w / wd).floor() as i32, (e / wd).floor() as i32);
        if (c0..=c1).any(|c| gaps.contains(&(c, row))) {
            return true;
        }
    }
    false
}

/// The fill of the gap sheets `gaps` from `o`'s source, those of `band` (longitudes w, e: the gap
/// sheets whose middle is in them are this run's, read near and their lakes kept; the rest are gaps
/// all the same, a lake's part in the gap sheets the same whichever band it falls in; None: all),
/// written to `out` content-named (`lakefill.<hash>.sect`) with `report.json`: what was done, and
/// each sheet's check (its source lakes, how many touch OSM's water). Bands' fills are put together
/// by `merge`. Returns the report.
pub fn fill(o: &FillOpts, gaps: &HashSet<nts::Key>, band: Option<(f64, f64)>, out: &Path) -> Result<serde_json::Value> {
    use crate::lakefill as lf;
    let t = std::time::Instant::now();
    let FillOpts { basemap: base, sources: srcs, field, keep, layer, intermittent, rules, confirm } = o;
    let (rules, confirm) = (*rules, *confirm);
    anyhow::ensure!(!srcs.is_empty(), "no source");
    std::fs::create_dir_all(out)?;
    // `--band w,e`: only the gap sheets whose middle is in those longitudes are this run's (read
    // near, and their lakes kept); the rest are gaps all the same (a lake's part in the gap sheets,
    // the same whichever band it falls in). Bands' fills are put together with `lakefill merge`.
    let mine: HashSet<nts::Key> = gaps
        .iter()
        .copied()
        .filter(|k| {
            let (w, _, e, _) = nts::bounds(*k);
            band.is_none_or(|(bw, be)| (bw..be).contains(&((w + e) / 2.0)))
        })
        .collect();
    eprintln!("lakefill: {} of them this run's", mine.len());
    // The source's pieces near the gap sheets (within `MARGIN`°: a lake joined from pieces is
    // checked whole below), read as they stream.
    const MARGIN: f64 = 1.0;
    let mut files = Vec::new();
    for s in srcs {
        files.extend(shapefiles(s, layer)?);
    }
    anyhow::ensure!(!files.is_empty(), "no {layer} shapefiles in {srcs:?}");
    let mut pieces = Vec::new();
    let mut read = Vec::new();
    let mut by_value: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    // (Four files at a time: each streams through its own unzip; the NAS gives more than one stream
    // takes. Pieces kept in the files' order.)
    type Read1 = (Vec<lf::Lake>, std::collections::BTreeMap<String, u64>, serde_json::Value);
    let read_one = |(shp_src, dbf_src, prj_src): &(Src, Src, Src)| -> Result<Read1> {
        let mut pieces = Vec::new();
        let mut by_value: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        let from = shp_src.name();
        let prj = prj_src.read_to_string().unwrap_or_default();
        anyhow::ensure!(prj.is_empty() || prj.starts_with("GEOGCS"), "{from}: not in longitude and latitude ({prj})");
        let sr = shp_src.open()?;
        let mut shp = crate::lakefill::shp::ShpReader::new(sr)?;
        // (A file nowhere near this run's gap sheets isn't read.)
        if !near_gaps(&mine, shp.bbox, MARGIN) {
            eprintln!("lakefill: {from}: not near ({:?})", shp.bbox);
            return Ok((pieces, by_value, serde_json::json!({"file": from, "skipped": "not near the gap sheets"})));
        }
        let mut dr = dbf_src.open()?;
        let mut dbf = crate::lakefill::shp::DbfReader::new(&mut dr)?;
        let col = dbf.col(field).with_context(|| format!("{from}: no field {field} (fields: {:?})", dbf.fields))?;
        let icol = match intermittent {
            Some((f, _)) => Some(dbf.col(f).with_context(|| format!("{from}: no field {f}"))?),
            None => None,
        };
        let (mut records, mut n, mut far) = (0u64, 0u64, 0u64);
        while let Some(rings) = shp.next_polygon()? {
            let row = dbf.next_row()?.with_context(|| format!("{from}: fewer rows than records"))?;
            records += 1;
            *by_value.entry(row[col].clone()).or_default() += 1;
            if !keep.iter().any(|k| *k == row[col]) || rings.is_empty() {
                continue;
            }
            let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for p in rings.iter().flatten() {
                b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
            }
            if !near_gaps(&mine, b, MARGIN) {
                far += 1;
                continue;
            }
            if let Some(mut l) = lf::Lake::from_lonlat(&rings) {
                l.intermittent = icol.zip(intermittent.as_ref()).is_some_and(|(c, (_, v))| row[c] == *v);
                pieces.push(l);
                n += 1;
            }
        }
        anyhow::ensure!(dbf.next_row()?.is_none(), "{from}: more rows than records");
        // (Each member read to its end, so its CRC is checked: a damaged zip fails here.)
        drop(dbf);
        std::io::copy(&mut dr, &mut std::io::sink()).with_context(|| format!("{from}: its table"))?;
        std::io::copy(&mut shp.into_inner(), &mut std::io::sink()).with_context(|| format!("{from}: its shapes"))?;
        eprintln!("lakefill: {from}: {records} records, {n} taken, {far} far from the gaps ({:.0} s)", t.elapsed().as_secs_f64());
        Ok((pieces, by_value, serde_json::json!({"file": from, "records": records, "taken": n, "far": far})))
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;
    let got: Vec<Read1> = pool.install(|| {
        use rayon::prelude::*;
        files.par_iter().map(read_one).collect::<Result<_>>()
    })?;
    for (p, v, i) in got {
        pieces.extend(p);
        for (k, n) in v {
            *by_value.entry(k).or_default() += n;
        }
        read.push(i);
    }
    eprintln!("lakefill: {} pieces read in {:.0} s; {field} values: {by_value:?}", pieces.len(), t.elapsed().as_secs_f64());
    let pm = store::pmtiles::PmTiles::with_leaf_cache(Box::new(store::range::PlainFile::open(base)?), 4096)?;
    let osm = lf::BasemapOsm::new(pm);
    let gap = |k: nts::Key| gaps.contains(&k);
    let mut sel = lf::select(pieces, &gap, &osm, &rules)?;
    // A lake up to MARGIN° across that meets a gap sheet lies within MARGIN° of it, and so does any
    // piece of it sharing an edge: all were read. A bigger one might have pieces unread: left out.
    let whole = |l: &lf::Lake| {
        let b = l.bbox();
        let (w, n) = lf::lonlat([b[0], b[1]]);
        let (e, s) = lf::lonlat([b[2], b[3]]);
        e - w < MARGIN && n - s < MARGIN
    };
    let too_big = sel.lakes.iter().zip(&sel.sheet_of).filter(|(l, k)| mine.contains(k) && !whole(l)).count();
    let (lakes, of): (Vec<lf::Lake>, Vec<nts::Key>) = sel.lakes.into_iter().zip(sel.sheet_of).filter(|(l, k)| whole(l) && mine.contains(k)).unzip();
    sel.sheets.retain(|k, _| mine.contains(k));
    (sel.lakes, sel.sheet_of) = (lakes, of);
    eprintln!("lakefill: {too_big} lakes over {MARGIN}° across left out (not read whole)");
    eprintln!("lakefill: {} in {:.0} s", serde_json::to_string(&sel.stats)?, t.elapsed().as_secs_f64());
    // Sheets confirmed as gaps: few of the source's lakes there touch OSM's water.
    let confirmed = |k: &nts::Key| match (confirm, sel.sheets.get(k)) {
        (Some(c), Some(s)) => f64::from(s.touching - s.completed) < c * f64::from(s.source),
        _ => true,
    };
    if confirm.is_some() {
        let (lakes, of): (Vec<lf::Lake>, Vec<nts::Key>) = sel.lakes.into_iter().zip(sel.sheet_of).filter(|(_, k)| confirmed(k)).unzip();
        sel.lakes = lakes;
        sel.sheet_of = of;
    }
    // The fill, content-named.
    let meta = serde_json::json!({"source": "CanVec (Natural Resources Canada), Open Government Licence – Canada", "layer": layer, "field": field, "keep": keep, "intermittent": intermittent, "rules": rules, "gap_sheets": gaps.len(), "band": band});
    let tmpf = out.join("lakefill.tmp");
    lf::write(&tmpf, &sel.lakes, meta)?;
    let h = store::naming::hash16_file(&tmpf)?;
    let name = format!("lakefill.{h}.sect");
    std::fs::rename(&tmpf, out.join(&name))?;
    let f = lf::Fill::open(&out.join(&name), &name)?;
    let sheets: Vec<serde_json::Value> = sel
        .sheets
        .iter()
        .map(|(k, c)| serde_json::json!({"sheet": nts::name(*k), "bounds": nts::bounds(*k), "source": c.source, "touching": c.touching, "kept": c.kept, "completed": c.completed, "filled": confirmed(k)}))
        .collect();
    let kept_by_sheet = sel.sheet_of.len();
    let report = serde_json::json!({"fill": name, "bytes": std::fs::metadata(out.join(&name))?.len(), "lakes": f.lakes().len(), "kept_after_confirm": kept_by_sheet, "too_big": too_big, "stats": sel.stats, "confirm": confirm, "read": read, "values": by_value, "sheets": sheets, "secs": t.elapsed().as_secs_f64()});
    std::fs::write(out.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    eprintln!("lakefill: {name}, {} lakes, in {:.0} s", f.lakes().len(), t.elapsed().as_secs_f64());
    Ok(report)
}

/// Bands' fills (`fill`, each run's folder in `runs`) as one, content-named, in `out`, with the
/// runs' reports together (`report.json`). Returns the fill's file and the report.
pub fn merge(runs: &[PathBuf], out: &Path) -> Result<(PathBuf, serde_json::Value)> {
    use crate::lakefill as lf;
    std::fs::create_dir_all(out)?;
    let mut lakes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut meta = serde_json::Value::Null;
    let mut sheets = Vec::new();
    let mut read = Vec::new();
    let mut stats: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for r in runs {
        let dir = r.as_path();
        let f = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "sect")).with_context(|| format!("{}: no fill", r.display()))?;
        let fill = lf::Fill::open(&f, "run")?;
        for i in 0..fill.lakes().len() as u32 {
            if seen.insert(fill.lakes()[i as usize].id) {
                lakes.push(fill.lake(i));
            }
        }
        if meta.is_null() {
            meta = fill.meta().clone();
        }
        let rep: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("report.json"))?)?;
        sheets.extend(rep["sheets"].as_array().cloned().unwrap_or_default());
        read.push(serde_json::json!({"run": r.display().to_string(), "read": rep["read"], "band": fill.meta()["band"], "too_big": rep["too_big"]}));
    }
    // (Totals from the sheets: each band's are its own, where its stats count its neighbours' too.)
    for sh in &sheets {
        for k in ["source", "touching", "kept", "completed"] {
            *stats.entry(k.to_string()).or_default() += sh[k].as_u64().unwrap_or(0);
        }
        *stats.entry(if sh["filled"].as_bool() == Some(false) { "sheets_left" } else { "sheets_filled" }.to_string()).or_default() += 1;
    }
    if let Some(o) = meta.as_object_mut() {
        for k in ["fmt", "grid_z", "lakes", "rings", "points", "band"] {
            o.remove(k);
        }
    }
    let tmpf = out.join("lakefill.tmp");
    lf::write(&tmpf, &lakes, meta)?;
    let h = store::naming::hash16_file(&tmpf)?;
    let name = format!("lakefill.{h}.sect");
    std::fs::rename(&tmpf, out.join(&name))?;
    let f = lf::Fill::open(&out.join(&name), &name)?;
    let report = serde_json::json!({"fill": name, "bytes": std::fs::metadata(out.join(&name))?.len(), "lakes": f.lakes().len(), "stats": stats, "runs": read, "sheets": sheets});
    std::fs::write(out.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    eprintln!("lakefill: {name}, {} lakes from {} runs", f.lakes().len(), runs.len());
    Ok((out.join(&name), report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declared lake-fill source (producers/sources/canvec-lakes.toml) fills by what the code
    /// had before it was declared (CanVec's `waterbody_2`, `definit` 20, 83, 86 and 87, `perm_en`
    /// Intermittent, sheets confirmed at a half), so the fill is the same; and while it's accepted
    /// as staged, the step's key is as it was (its zips' pin alone).
    #[test]
    fn the_declared_source_fills_as_the_code_did() {
        let s = crate::inputs::sources::staged();
        let src = s.role(crate::inputs::sources::LAKE_FILL).unwrap();
        let o = FillOpts::declared(Path::new("b.pmtiles"), Path::new("zips"), src).unwrap();
        assert_eq!((o.layer.as_str(), o.field.as_str(), o.keep.clone(), o.intermittent.clone(), o.confirm), ("waterbody_2", "definit", ["20", "83", "86", "87"].map(String::from).to_vec(), Some(("perm_en".to_string(), "Intermittent".to_string())), Some(0.5)));
        assert_eq!(serde_json::to_value(o.rules).unwrap(), serde_json::to_value(super::super::Rules::default()).unwrap());
        assert_eq!(source_dir(src), "sources/fetched/canvec-lakes");
        assert_eq!(src.digest(), crate::agent::rekey::declared_before::LAKE_FILL);
        assert_eq!(src.key_extra(crate::agent::rekey::declared_before::LAKE_FILL), None);
        let mut other = src.clone();
        other.data.params.insert("keep".into(), serde_json::json!(["83"]));
        assert_eq!(other.key_extra(crate::agent::rekey::declared_before::LAKE_FILL), Some(format!("source canvec-lakes {}", src.keyed)));
    }

    /// The step's key names the declared source's zips by their pin alone, as it did before the
    /// source was declared (agent::build::lakefill_work), and its folder is the declared one.
    #[test]
    fn the_lake_fills_source_as_planned() {
        let d = tempfile::tempdir().unwrap();
        let mut m = std::collections::BTreeMap::new();
        assert!(planned_source(d.path(), &m).unwrap_err().contains("no accepted version"));
        crate::inputs::sources::accept_fixture(d.path(), &mut m);
        // (Declared, its folder not there: an error, which holds the water, terrain and catalog.)
        assert!(planned_source(d.path(), &m).unwrap_err().contains("isn't there"));
        let z = d.path().join("sources/fetched/canvec-lakes");
        std::fs::create_dir_all(&z).unwrap();
        assert_eq!(planned_source(d.path(), &m), Ok(None), "no zips: no fill");
        std::fs::write(z.join("canvec_50K_AB_Hydro_shp.zip"), b"zip").unwrap();
        assert_eq!(planned_source(d.path(), &m).unwrap(), source_pin(&z).unwrap());
    }

    /// A survey row for the 0.5° × 0.25° sheet at column `c`, row `r` (south-west at 60°W + c/2,
    /// 50°N + r/4), all land, with `lakes`.
    fn row(c: i32, r: i32, lakes: [u32; 4]) -> SheetRow {
        let (w, s) = (-100.0 + f64::from(c) * 0.5, 50.0 + f64::from(r) * 0.25);
        SheetRow { sheet: format!("{c}/{r}"), west: w, south: s, east: w + 0.5, north: s + 0.25, region: "test".into(), land_km2: 0.0, sea_km2: 0.0, inland_km2: 0.0, lakes, z12_bytes: 0, z12_tiles: 0 }
    }

    const FULL: [u32; 4] = [500, 300, 100, 10];

    /// An 11 × 11 grid of sheets full of lakes, with `odd` ones in place.
    fn grid(odd: &[((i32, i32), [u32; 4])]) -> Vec<SheetRow> {
        let mut v = Vec::new();
        for r in 0..11 {
            for c in 0..11 {
                let l = odd.iter().find(|(k, _)| *k == (c, r)).map_or(FULL, |(_, l)| *l);
                v.push(row(c, r, l));
            }
        }
        v
    }

    fn found(rows: &[SheetRow]) -> Vec<(String, Vec<&'static str>)> {
        gaps(rows).into_iter().map(|g| (g.row.sheet, g.why)).collect()
    }

    #[test]
    fn a_sheet_full_of_lakes_among_its_like_is_no_gap() {
        assert!(gaps(&grid(&[])).is_empty());
    }

    #[test]
    fn empty_sparse_and_cut_off_sheets_are_gaps_and_why() {
        let rows = grid(&[
            // No small lakes: empty, and far under its neighbours.
            ((2, 2), [0, 0, 0, 0]),
            // A few: under a tenth of its neighbours' (about 130 a 100 km²), over 2.
            ((8, 2), [30, 20, 10, 0]),
            // Big lakes but no small ones: its size curve cut off (and empty).
            ((5, 8), [0, 0, 1, 4]),
        ]);
        assert_eq!(found(&rows), [("2/2".into(), vec!["neighbours", "empty"]), ("8/2".into(), vec!["neighbours"]), ("5/8".into(), vec!["neighbours", "empty", "truncated"])]);
    }

    #[test]
    fn the_neighbours_test_reaches_inside_a_block_of_gaps() {
        // A 5 × 5 block of sparse sheets: the middle one's neighbours are mostly the block's, but
        // with the gaps found left out its reference is the full sheets'.
        let mut odd = Vec::new();
        for r in 3..8 {
            for c in 3..8 {
                odd.push(((c, r), [30, 20, 10, 0]));
            }
        }
        let g = found(&grid(&odd));
        assert_eq!(g.len(), 25, "{g:?}");
        assert!(g.iter().all(|(_, why)| *why == ["neighbours"]));
    }

    #[test]
    fn a_sheet_mostly_water_isnt_judged() {
        let mut rows = grid(&[((4, 4), [0, 0, 0, 0])]);
        let k = rows.iter().position(|r| r.sheet == "4/4").unwrap();
        rows[k].inland_km2 = 940.0;
        assert!(gaps(&rows).is_empty());
    }

    #[test]
    fn the_survey_and_gap_lists_read_back() {
        let mut rows = grid(&[((2, 2), [0, 0, 0, 0])]);
        rows[3].sea_km2 = 12.3456;
        let text = sheets_csv(&rows);
        let back = read_sheets_csv(&text).unwrap();
        assert_eq!(back.len(), rows.len());
        assert_eq!(back[3].sea_km2, 12.346, "to the metre², as the CSV has it");
        assert_eq!(back[5], rows[5]);
        let g = gaps(&back);
        let keys = read_gaps_csv(&gaps_csv(&g)).unwrap();
        assert_eq!(keys, g.iter().map(|v| v.row.key()).collect());
        assert_eq!(keys.len(), 1);
        assert_eq!(nts::bounds(*keys.iter().next().unwrap()), (-99.0, 50.5, -98.5, 50.75));
        assert!(read_sheets_csv("sheet,west\n").is_err());
    }

    #[test]
    fn the_sources_pin_is_its_zips_names_and_sizes() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(source_pin(d.path()).unwrap(), None);
        assert_eq!(source_pin(&d.path().join("none")).unwrap(), None);
        let set = |name: &str, bytes: &[u8], t: u64| {
            let p = d.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t)).unwrap();
        };
        set("b_Hydro_shp.zip", b"12345", 1_700_000_000);
        set("a_Hydro_shp.zip", b"1", 1_700_000_100);
        set("notes.txt", b"x", 1);
        set("._a_Hydro_shp.zip", b"xx", 1);
        assert_eq!(source_pin(d.path()).unwrap().as_deref(), Some("a_Hydro_shp.zip 1 1700000100\nb_Hydro_shp.zip 5 1700000000"));
        // The same size, replaced: another source.
        set("a_Hydro_shp.zip", b"2", 1_700_000_200);
        assert_eq!(source_pin(d.path()).unwrap().as_deref(), Some("a_Hydro_shp.zip 1 1700000200\nb_Hydro_shp.zip 5 1700000000"));
        // A folder that can't be read is an error, not "no source".
        let f = d.path().join("notes.txt");
        assert!(source_pin(&f).is_err());
    }

    #[test]
    fn the_water_waits_for_the_fill_until_it_has_failed_a_few_times() {
        use crate::agent::build::{water_waits_for_lakefill as waits, LAKEFILL_HOLDS};
        assert!(waits(true, 0) && waits(true, LAKEFILL_HOLDS - 1));
        assert!(!waits(true, LAKEFILL_HOLDS) && !waits(true, LAKEFILL_HOLDS + 5));
        assert!(!waits(false, 0), "no lakefill work: nothing to wait for");
    }

    #[test]
    fn the_fill_runs_once_a_pass_and_source_and_the_water_and_terrain_name_it() {
        use crate::agent::build;
        let date = "2026-09-28";
        let mut m = std::collections::BTreeMap::new();
        let done = build::Keys::default();
        // Without the pass's basemap and outlines, or the source: nothing.
        assert!(build::lakefill_work(date, &m, &done, Some("a.zip 1")).is_none());
        m.insert(format!("layers/basemap/world-{date}"), "layers/basemap/world-x.0123456789abcdef.pmtiles".to_string());
        m.insert(format!("sources/osm/{date}/outlines"), "sources/osm/x/outlines.0123456789abcdef.sect".to_string());
        assert!(build::lakefill_work(date, &m, &done, None).is_none());
        let w = build::lakefill_work(date, &m, &done, Some("a.zip 1")).unwrap();
        assert_eq!(w.step, "lakefill");
        let mut done2 = build::Keys::default();
        done2.record("lakefill", &w.targets);
        assert!(build::lakefill_work(date, &m, &done2, Some("a.zip 1")).is_none(), "done");
        assert!(build::lakefill_work(date, &m, &done2, Some("a.zip 2")).is_some(), "a zip replaced");
        // The water's key and the terrain's water pin take the fill once the manifest has it.
        let water = build::water_work(date, &m, &done).unwrap().targets[0].1.clone();
        let pin = crate::terrain_pack::water_source_pin(&m).unwrap();
        assert_eq!(pin, "layers/basemap/world-x.0123456789abcdef.pmtiles");
        m.insert(crate::lakefill::logical(date), "work/lakefill/2026-09-28.0123456789abcdef.sect".to_string());
        let filled = build::water_work(date, &m, &done).unwrap().targets[0].1.clone();
        assert_ne!(filled, water);
        assert_eq!(crate::terrain_pack::water_source_pin(&m).unwrap(), "layers/basemap/world-x.0123456789abcdef.pmtiles + work/lakefill/2026-09-28.0123456789abcdef.sect");
        // A newer pass's basemap: the older pass's fill is drawn by neither, until one is made from it.
        let newer = "2027-03-01";
        m.insert(format!("layers/basemap/world-{newer}"), "layers/basemap/world-y.fedcba9876543210.pmtiles".to_string());
        assert_eq!(crate::lakefill::current(&m), None);
        assert_eq!(crate::terrain_pack::water_source_pin(&m).unwrap(), "layers/basemap/world-y.fedcba9876543210.pmtiles");
        let w_new = build::water_work(newer, &m, &done).unwrap().targets[0].1.clone();
        m.insert(crate::lakefill::logical(newer), "work/lakefill/2027-03-01.0123456789abcdef.sect".to_string());
        assert_ne!(build::water_work(newer, &m, &done).unwrap().targets[0].1, w_new);
        assert_eq!(crate::lakefill::current(&m).map(|(l, _)| l), Some(crate::lakefill::logical(newer)));
    }
}
