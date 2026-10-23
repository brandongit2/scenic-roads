//! Overture's buildings as GeoParquet (the parquet crate, through arrow's arrays): a row group's
//! rows meeting a box as `bldprep`'s columns ([`row_group`]), a file's row groups' boxes from its
//! footer's statistics ([`footer`], for bld-fetch), and every building's box ([`boxes`], for the
//! roadside buildings' scan).
//!
//! A row group is read as pyarrow read it for the Python step before: only the columns used (of `sources`, its
//! `property`, `dataset` and `record_id`), each checked to be the type Overture writes (or one
//! read the same way), its rows kept whose box meets the box asked for (a row with no box: not
//! kept), in the file's order; the strings coded in the order they first come (`-1` none).

use super::prep::{kind, Frame};
use anyhow::{bail, ensure, Context, Result};
use arrow_array::{
    Array, ArrayRef, BinaryArray, BooleanArray, Float32Array, Float64Array, Int32Array, Int64Array, LargeBinaryArray,
    LargeListArray, LargeStringArray, ListArray, RecordBatch, StringArray, StructArray,
};
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::arrow::ProjectionMask;
use parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use parquet::file::reader::ChunkReader;
use parquet::file::statistics::Statistics;
use std::collections::HashMap;

/// A building's columns read (`bldprep`).
const B_COLS: [&str; 13] = [
    "id",
    "sources",
    "height",
    "num_floors",
    "min_height",
    "min_floor",
    "class",
    "subtype",
    "roof_shape",
    "has_parts",
    "is_underground",
    "geometry",
    "bbox",
];
/// A building part's.
const P_COLS: [&str; 11] = [
    "id",
    "sources",
    "height",
    "num_floors",
    "min_height",
    "min_floor",
    "roof_shape",
    "is_underground",
    "geometry",
    "building_id",
    "bbox",
];
/// Of `sources`' fields, those read.
const SOURCE_FIELDS: [&str; 3] = ["property", "dataset", "record_id"];
const I32_NONE: i32 = i32::MIN;

/// The leaves under the columns `cols` (of `sources`, `SOURCE_FIELDS` alone), and the columns
/// missing.
fn leaves(schema: &parquet::schema::types::SchemaDescriptor, cols: &[&str]) -> (Vec<usize>, Vec<String>) {
    let mut found = vec![false; cols.len()];
    let mut out = Vec::new();
    for i in 0..schema.num_columns() {
        let path = schema.column(i).path().parts().to_vec();
        let Some(k) = cols.iter().position(|c| path[0] == *c) else { continue };
        if path[0] == "sources" && !path.last().is_some_and(|l| SOURCE_FIELDS.contains(&l.as_str())) {
            continue;
        }
        found[k] = true;
        out.push(i);
    }
    let mut missing: Vec<String> = cols.iter().zip(&found).filter(|x| !x.1).map(|x| x.0.to_string()).collect();
    missing.sort();
    (out, missing)
}

// ---- arrow's arrays, as the columns' types read ---------------------------------------------------

/// A string column (utf8, either offset width).
enum Strs<'a> {
    S(&'a StringArray),
    L(&'a LargeStringArray),
}

impl<'a> Strs<'a> {
    fn of(a: &'a dyn Array) -> Option<Strs<'a>> {
        let any = a.as_any();
        any.downcast_ref::<StringArray>().map(Strs::S).or_else(|| any.downcast_ref::<LargeStringArray>().map(Strs::L))
    }
    fn get(&self, i: usize) -> Option<&'a str> {
        match self {
            Strs::S(a) => a.is_valid(i).then(|| a.value(i)),
            Strs::L(a) => a.is_valid(i).then(|| a.value(i)),
        }
    }
}

/// A binary column.
enum Bins<'a> {
    S(&'a BinaryArray),
    L(&'a LargeBinaryArray),
}

impl<'a> Bins<'a> {
    fn of(a: &'a dyn Array) -> Option<Bins<'a>> {
        let any = a.as_any();
        any.downcast_ref::<BinaryArray>().map(Bins::S).or_else(|| any.downcast_ref::<LargeBinaryArray>().map(Bins::L))
    }
    fn get(&self, i: usize) -> &'a [u8] {
        match self {
            Bins::S(a) if a.is_valid(i) => a.value(i),
            Bins::L(a) if a.is_valid(i) => a.value(i),
            _ => &[],
        }
    }
}

/// A floating-point column's value (None: null).
fn float(a: &dyn Array, i: usize) -> Option<f64> {
    if a.is_null(i) {
        return None;
    }
    let any = a.as_any();
    any.downcast_ref::<Float64Array>()
        .map(|x| x.value(i))
        .or_else(|| any.downcast_ref::<Float32Array>().map(|x| x.value(i) as f64))
}

fn is_float(a: &dyn Array) -> bool {
    a.as_any().is::<Float64Array>() || a.as_any().is::<Float32Array>()
}

/// An integer column's value as numpy's `astype("<i4")` has it (wrapping); None: null.
fn int32(a: &dyn Array, i: usize) -> Option<i32> {
    if a.is_null(i) {
        return None;
    }
    let any = a.as_any();
    any.downcast_ref::<Int32Array>()
        .map(|x| x.value(i))
        .or_else(|| any.downcast_ref::<Int64Array>().map(|x| x.value(i) as i32))
}

fn is_int(a: &dyn Array) -> bool {
    a.as_any().is::<Int32Array>() || a.as_any().is::<Int64Array>()
}

/// A list column: each row's span of its values (an empty one for a null row), and its values.
fn list(a: &dyn Array) -> Option<(Vec<(usize, usize)>, ArrayRef)> {
    let any = a.as_any();
    let span = |valid: bool, s: usize, e: usize| if valid { (s, e) } else { (s, s) };
    if let Some(l) = any.downcast_ref::<ListArray>() {
        let o = l.value_offsets();
        return Some((
            (0..l.len()).map(|i| span(l.is_valid(i), o[i] as usize, o[i + 1] as usize)).collect(),
            l.values().clone(),
        ));
    }
    let l = any.downcast_ref::<LargeListArray>()?;
    let o = l.value_offsets();
    Some(((0..l.len()).map(|i| span(l.is_valid(i), o[i] as usize, o[i + 1] as usize)).collect(), l.values().clone()))
}

/// Whether a column's type is the one read (Overture's, or one read the same way): a float column
/// read as integers would be truncated, a string one as numbers fail late.
fn type_ok(name: &str, a: &dyn Array) -> bool {
    let strs = |a: &dyn Array| Strs::of(a).is_some();
    let floats =
        |s: &StructArray, f: &[&str]| f.iter().all(|k| s.column_by_name(k).is_some_and(|c| is_float(c.as_ref())));
    match name {
        "id" | "building_id" | "class" | "subtype" | "roof_shape" => strs(a),
        "height" | "min_height" => is_float(a),
        "num_floors" | "min_floor" => is_int(a),
        "has_parts" | "is_underground" => a.as_any().is::<BooleanArray>(),
        "geometry" => Bins::of(a).is_some(),
        "bbox" => {
            a.as_any().downcast_ref::<StructArray>().is_some_and(|s| floats(s, &["xmin", "xmax", "ymin", "ymax"]))
        }
        "sources" => list(a).is_some_and(|(_, v)| {
            v.as_any()
                .downcast_ref::<StructArray>()
                .is_some_and(|s| SOURCE_FIELDS.iter().all(|k| s.column_by_name(k).is_some_and(|c| strs(c.as_ref()))))
        }),
        _ => false,
    }
}

/// Strings coded in the order they first come (pyarrow's `dictionary_encode`).
#[derive(Default)]
struct Codes {
    names: Vec<String>,
    by: HashMap<String, i32>,
}

impl Codes {
    fn code(&mut self, s: Option<&str>) -> i32 {
        let Some(s) = s else { return -1 };
        if let Some(&c) = self.by.get(s) {
            return c;
        }
        let c = self.names.len() as i32;
        self.names.push(s.to_string());
        self.by.insert(s.to_string(), c);
        c
    }
}

/// A row group's kept rows, column by column, as bldprep's frames hold them.
#[derive(Default)]
struct Cols {
    n: usize,
    id: Vec<u8>,
    geom_off: Vec<u64>,
    geom: Vec<u8>,
    height: Vec<f64>,
    min_height: Vec<f64>,
    num_floors: Vec<i32>,
    min_floor: Vec<i32>,
    roof: Vec<i32>,
    is_underground: Vec<u8>,
    fds: Vec<i32>,
    hds: Vec<i32>,
    rid_off: Vec<u64>,
    rid: Vec<u8>,
    class: Vec<i32>,
    subtype: Vec<i32>,
    has_parts: Vec<u8>,
    parent: Vec<u8>,
    roofs: Codes,
    datasets: Codes,
    classes: Codes,
    subtypes: Codes,
}

/// A UUID column's value as its 36 bytes (none: 36 zeros, which aren't a UUID).
fn id36(out: &mut Vec<u8>, s: Option<&str>) -> Result<()> {
    let s = s.unwrap_or("000000000000000000000000000000000000");
    ensure!(s.chars().count() == 36, "ids aren't UUIDs: {s:?}");
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

impl Cols {
    /// Adds a batch's rows whose box meets `b` (w, s, e, n).
    fn add(&mut self, batch: &RecordBatch, part: bool, b: [f64; 4]) -> Result<()> {
        let col = |n: &str| batch.column_by_name(n).with_context(|| format!("no column {n}"));
        let bbox = col("bbox")?.as_any().downcast_ref::<StructArray>().context("bbox")?;
        let bf = |k: &str| bbox.column_by_name(k).map(|c| c.as_ref()).context("bbox's fields");
        let (xmin, xmax, ymin, ymax) = (bf("xmin")?, bf("xmax")?, bf("ymin")?, bf("ymax")?);
        let ids = Strs::of(col("id")?.as_ref()).context("id")?;
        let geom = Bins::of(col("geometry")?.as_ref()).context("geometry")?;
        let (height, min_height) = (col("height")?.as_ref(), col("min_height")?.as_ref());
        let (floors, min_floor) = (col("num_floors")?.as_ref(), col("min_floor")?.as_ref());
        let roof = Strs::of(col("roof_shape")?.as_ref()).context("roof_shape")?;
        let under = col("is_underground")?.as_any().downcast_ref::<BooleanArray>().context("is_underground")?;
        let (spans, srcs) = list(col("sources")?.as_ref()).context("sources")?;
        let srcs = srcs.as_any().downcast_ref::<StructArray>().context("sources")?;
        let sf = |k: &str| srcs.column_by_name(k).and_then(|c| Strs::of(c.as_ref())).context("sources' fields");
        let (prop, dataset, record) = (sf("property")?, sf("dataset")?, sf("record_id")?);
        let bools = |a: &BooleanArray, i: usize| u8::from(a.is_valid(i) && a.value(i));
        let (class, subtype, has_parts, parent) = if part {
            (None, None, None, Some(Strs::of(col("building_id")?.as_ref()).context("building_id")?))
        } else {
            (
                Some(Strs::of(col("class")?.as_ref()).context("class")?),
                Some(Strs::of(col("subtype")?.as_ref()).context("subtype")?),
                Some(col("has_parts")?.as_any().downcast_ref::<BooleanArray>().context("has_parts")?),
                None,
            )
        };
        if self.geom_off.is_empty() {
            self.geom_off.push(0);
            self.rid_off.push(0);
        }
        for i in 0..batch.num_rows() {
            let v = |a: &dyn Array| if bbox.is_valid(i) { float(a, i) } else { None };
            let keep = match (v(xmin), v(xmax), v(ymin), v(ymax)) {
                (Some(x0), Some(x1), Some(y0), Some(y1)) => x0 <= b[2] && x1 >= b[0] && y0 <= b[3] && y1 >= b[1],
                _ => false,
            };
            if !keep {
                continue;
            }
            self.n += 1;
            id36(&mut self.id, ids.get(i))?;
            self.geom.extend_from_slice(geom.get(i));
            self.geom_off.push(self.geom.len() as u64);
            self.height.push(float(height, i).unwrap_or(f64::NAN));
            self.min_height.push(float(min_height, i).unwrap_or(f64::NAN));
            self.num_floors.push(int32(floors, i).unwrap_or(I32_NONE));
            self.min_floor.push(int32(min_floor, i).unwrap_or(I32_NONE));
            self.roof.push(self.roofs.code(roof.get(i)));
            self.is_underground.push(bools(under, i));
            // Sources: the footprint's (property "" or none) and the height's, the first of each;
            // every source's dataset coded, in order.
            let (mut fds, mut hds, mut rid) = (-1, None, None);
            for k in spans[i].0..spans[i].1 {
                let d = self.datasets.code(dataset.get(k));
                let p = prop.get(k);
                if p.is_none_or(str::is_empty) && rid.is_none() {
                    fds = d;
                    rid = Some(record.get(k).unwrap_or(""));
                }
                if p == Some("/properties/height") && hds.is_none() {
                    hds = Some(d);
                }
            }
            let hds = hds.unwrap_or(-1);
            self.fds.push(fds);
            self.hds.push(hds);
            self.rid.extend_from_slice(rid.unwrap_or("").as_bytes());
            self.rid_off.push(self.rid.len() as u64);
            if let (Some(c), Some(s), Some(h)) = (&class, &subtype, has_parts) {
                self.class.push(self.classes.code(c.get(i)));
                self.subtype.push(self.subtypes.code(s.get(i)));
                self.has_parts.push(bools(h, i));
            }
            if let Some(p) = &parent {
                id36(&mut self.parent, p.get(i))?;
            }
        }
        Ok(())
    }

    fn frame(self, part: bool, src: &str) -> Frame {
        let le = |v: &[u8]| v.to_vec();
        let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let i32s = |v: &[i32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let u64s = |v: &[u64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let mut cols: Vec<(&str, Vec<u8>)> = vec![
            ("id", le(&self.id)),
            ("geom_off", u64s(&self.geom_off)),
            ("geom", le(&self.geom)),
            ("height", f64s(&self.height)),
            ("min_height", f64s(&self.min_height)),
            ("num_floors", i32s(&self.num_floors)),
            ("min_floor", i32s(&self.min_floor)),
            ("roof", i32s(&self.roof)),
            ("is_underground", le(&self.is_underground)),
            ("fds", i32s(&self.fds)),
            ("hds", i32s(&self.hds)),
            ("rid_off", u64s(&self.rid_off)),
            ("rid", le(&self.rid)),
        ];
        if part {
            cols.push(("parent", le(&self.parent)));
        } else {
            cols.extend([
                ("class", i32s(&self.class)),
                ("subtype", i32s(&self.subtype)),
                ("has_parts", le(&self.has_parts)),
            ]);
        }
        let header = serde_json::json!({
            "n": self.n,
            "src": src,
            "dicts": {
                "roof": self.roofs.names,
                "dataset": self.datasets.names,
                "class": self.classes.names,
                "subtype": self.subtypes.names,
            },
        });
        Frame {
            kind: if part { kind::PARTS } else { kind::BUILDINGS },
            header,
            cols: cols.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }
}

/// A downloaded file's footer, read once for its row groups.
pub fn open(file: &std::fs::File) -> Result<ArrowReaderMetadata> {
    Ok(ArrowReaderMetadata::load(file, ArrowReaderOptions::new())?)
}

/// Row group `rg` of the file at `path` (its footer `meta`): its rows whose box meets `b` (w, s, e,
/// n), as a frame of buildings or (`part`) building parts; None when none does. `src` names it in
/// errors. (The file opened for this read alone: a parquet reader seeks its handle, which clones
/// share.)
pub fn row_group(
    path: &std::path::Path,
    meta: &ArrowReaderMetadata,
    rg: usize,
    part: bool,
    b: [f64; 4],
    src: &str,
) -> Result<Option<Frame>> {
    let want: &[&str] = if part { &P_COLS } else { &B_COLS };
    let (idx, missing) = leaves(meta.metadata().file_metadata().schema_descr(), want);
    if !missing.is_empty() {
        bail!("{src}: Overture's columns changed: no {missing:?}");
    }
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::new_with_metadata(file, meta.clone());
    let mask = ProjectionMask::leaves(builder.parquet_schema(), idx);
    let rows = meta.metadata().row_group(rg).num_rows().max(1) as usize;
    let reader = builder.with_row_groups(vec![rg]).with_projection(mask).with_batch_size(rows).build()?;
    let mut c = Cols::default();
    let mut checked = false;
    for batch in reader {
        let batch = batch.with_context(|| format!("{src}: read"))?;
        if !checked {
            let mut retyped: Vec<String> = Vec::new();
            let mut names: Vec<&str> = want.to_vec();
            names.sort();
            for n in names {
                let a = batch.column_by_name(n).with_context(|| format!("{src}: no column {n}"))?;
                if !type_ok(n, a.as_ref()) {
                    retyped.push(format!("{n} ({})", a.data_type()));
                }
            }
            if !retyped.is_empty() {
                bail!("{src}: Overture's columns changed type: {}", retyped.join(", "));
            }
            checked = true;
        }
        c.add(&batch, part, b).with_context(|| src.to_string())?;
    }
    Ok((c.n > 0).then(|| c.frame(part, src)))
}

// ---- footers -------------------------------------------------------------------------------------

/// The bytes a parquet file's footer needs, from its last 64 KiB (`tail`): its length with the
/// 8 bytes after it, when more than the tail holds.
pub fn footer_need(tail: &[u8]) -> Result<Option<u64>> {
    ensure!(tail.len() >= 8 && tail.ends_with(b"PAR1"), "not a parquet file");
    let need = u32::from_le_bytes(tail[tail.len() - 8..tail.len() - 4].try_into().unwrap()) as u64 + 8;
    Ok((need > tail.len() as u64).then_some(need))
}

/// A file's footer (its last bytes, the footer whole) as its metadata.
pub fn footer_meta(tail: &[u8]) -> Result<ParquetMetaData> {
    let need = footer_need(tail)?;
    ensure!(need.is_none(), "the footer cut short");
    let len = u32::from_le_bytes(tail[tail.len() - 8..tail.len() - 4].try_into().unwrap()) as usize;
    Ok(ParquetMetaDataReader::decode_metadata(&tail[tail.len() - 8 - len..tail.len() - 8])?)
}

/// A file's rows and its row groups' boxes `[w, s, e, n]` with their rows, from its footer's
/// statistics (`bbox.xmin` … `bbox.ymax`); a row group without them is the whole world.
pub fn row_group_boxes(md: &ParquetMetaData) -> Result<(i64, Vec<([f64; 4], i64)>)> {
    let schema = md.file_metadata().schema_descr();
    let col = |name: &str| (0..schema.num_columns()).find(|&i| schema.column(i).path().string() == name);
    let names = ["bbox.xmin", "bbox.ymin", "bbox.xmax", "bbox.ymax"];
    let cols: Vec<Option<usize>> = names.iter().map(|n| col(n)).collect();
    let mut rgs = Vec::new();
    for g in md.row_groups() {
        let mm = |k: usize| -> Option<(f64, f64)> {
            match g.column(cols[k]?).statistics()? {
                Statistics::Double(s) => Some((*s.min_opt()?, *s.max_opt()?)),
                Statistics::Float(s) => Some((*s.min_opt()? as f64, *s.max_opt()? as f64)),
                _ => None,
            }
        };
        let st: Vec<Option<(f64, f64)>> = (0..4).map(mm).collect();
        let b = match st[..] {
            [Some(a), Some(b), Some(c), Some(d)] => [a.0, b.0, c.1, d.1],
            _ => [-180.0, -90.0, 180.0, 90.0],
        };
        rgs.push((b, g.num_rows()));
    }
    Ok((md.file_metadata().num_rows(), rgs))
}

// ---- every building's box ------------------------------------------------------------------------

/// Byte ranges of a file held in memory (the bbox columns' chunks fetched): a parquet reader of
/// those alone.
pub struct Spans {
    len: u64,
    spans: std::collections::BTreeMap<u64, bytes::Bytes>,
}

impl Spans {
    pub fn new(len: u64) -> Spans {
        Spans { len, spans: Default::default() }
    }

    pub fn add(&mut self, at: u64, b: Vec<u8>) {
        self.spans.insert(at, bytes::Bytes::from(b));
    }

    fn slice(&self, start: u64, len: usize) -> parquet::errors::Result<bytes::Bytes> {
        let miss = || parquet::errors::ParquetError::General(format!("bytes {start}..+{len} weren't fetched"));
        let (&at, b) = self.spans.range(..=start).next_back().ok_or_else(miss)?;
        let s = (start - at) as usize;
        if s + len > b.len() {
            return Err(miss());
        }
        Ok(b.slice(s..s + len))
    }
}

impl parquet::file::reader::Length for Spans {
    fn len(&self) -> u64 {
        self.len
    }
}

impl ChunkReader for Spans {
    type T = std::io::Cursor<bytes::Bytes>;

    fn get_read(&self, start: u64) -> parquet::errors::Result<Self::T> {
        let (&at, b) = self
            .spans
            .range(..=start)
            .next_back()
            .ok_or_else(|| parquet::errors::ParquetError::General(format!("byte {start} wasn't fetched")))?;
        Ok(std::io::Cursor::new(b.slice((start - at) as usize..)))
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<bytes::Bytes> {
        self.slice(start, length)
    }
}

/// The leaves of `bbox`'s four fields, and the byte span of each row group's chunks of them.
pub fn bbox_spans(md: &ParquetMetaData) -> Result<(Vec<usize>, Vec<(u64, u64)>)> {
    let schema = md.file_metadata().schema_descr();
    let idx: Vec<usize> = ["bbox.xmin", "bbox.ymin", "bbox.xmax", "bbox.ymax"]
        .iter()
        .map(|n| {
            (0..schema.num_columns())
                .find(|&i| schema.column(i).path().string() == *n)
                .with_context(|| format!("Overture's columns changed: no {n}"))
        })
        .collect::<Result<_>>()?;
    let spans = md
        .row_groups()
        .iter()
        .map(|g| {
            let r: Vec<(u64, u64)> = idx.iter().map(|&i| g.column(i).byte_range()).collect();
            let s = r.iter().map(|x| x.0).min().unwrap_or(0);
            let e = r.iter().map(|x| x.0 + x.1).max().unwrap_or(0);
            (s, e)
        })
        .collect();
    Ok((idx, spans))
}

/// Every row's box as f32 `[xmin, ymin, xmax, ymax]` (DuckDB's doubles cast as numpy casts them),
/// in the file's order, from a reader holding the bbox columns' chunks (`bbox_spans`).
pub fn boxes<R: ChunkReader + 'static>(reader: R, md: std::sync::Arc<ParquetMetaData>) -> Result<Vec<[f32; 4]>> {
    let meta = ArrowReaderMetadata::try_new(md, ArrowReaderOptions::new())?;
    let (idx, _) = bbox_spans(meta.metadata())?;
    let builder = ParquetRecordBatchReaderBuilder::new_with_metadata(reader, meta);
    let mask = ProjectionMask::leaves(builder.parquet_schema(), idx);
    let mut out = Vec::new();
    for batch in builder.with_projection(mask).with_batch_size(1 << 16).build()? {
        let batch = batch?;
        let bbox = batch
            .column_by_name("bbox")
            .and_then(|c| c.as_any().downcast_ref::<StructArray>())
            .context("Overture's columns changed: bbox")?;
        let f = |k: &str| -> Result<&dyn Array> {
            let c = bbox.column_by_name(k).with_context(|| format!("Overture's columns changed: bbox.{k}"))?;
            ensure!(is_float(c.as_ref()), "Overture's columns changed type: bbox.{k} ({})", c.data_type());
            Ok(c.as_ref())
        };
        let (a, b, c, d) = (f("xmin")?, f("ymin")?, f("xmax")?, f("ymax")?);
        for i in 0..batch.num_rows() {
            // (A null: NaN. Overture's boxes have none.)
            let v = |x: &dyn Array| if bbox.is_valid(i) { float(x, i).unwrap_or(f64::NAN) } else { f64::NAN } as f32;
            out.push([v(a), v(b), v(c), v(d)]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_footers_length_is_its_last_bytes() {
        let mut t = vec![0u8; 100];
        t.extend(200u32.to_le_bytes());
        t.extend(b"PAR1");
        assert_eq!(footer_need(&t).unwrap(), Some(208));
        let mut t = vec![0u8; 100];
        t.extend(20u32.to_le_bytes());
        t.extend(b"PAR1");
        assert_eq!(footer_need(&t).unwrap(), None);
        assert!(footer_need(b"nope").is_err());
    }
}
