//! GeoTIFF and Cloud Optimized GeoTIFF reading over byte ranges (`store::range::RangeRead`), as
//! GDAL reads them, for the elevation and land cover steps: NRCan's HRDEM and MRDEM (LZW), USGS
//! 3DEP (LZW, floating-point predictor), FABDEM (Deflate, horizontal predictor), Taiwan's MOI DTM
//! and ESA WorldCover (Deflate). Classic TIFF and BigTIFF in either byte order; tiles or strips;
//! overviews; no compression, LZW, Deflate or Zstandard; predictor 1, 2 or 3; 8- to 64-bit integer
//! and floating-point samples, of which band 1 is read. The georeferencing as GDAL computes it
//! (pixel scale and tie point, or the transformation matrix; a PixelIsPoint raster moved half a
//! pixel), the GeoKeys, and GDAL's nodata tag. And `write_f32` for FABDEM's store: one band,
//! tiled, Deflate, predictor 3.

use anyhow::{bail, ensure, Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use store::range::RangeRead;

const NEW_SUBFILE_TYPE: u16 = 254;
const IMAGE_WIDTH: u16 = 256;
const IMAGE_LENGTH: u16 = 257;
const BITS_PER_SAMPLE: u16 = 258;
const COMPRESSION: u16 = 259;
const STRIP_OFFSETS: u16 = 273;
const SAMPLES_PER_PIXEL: u16 = 277;
const ROWS_PER_STRIP: u16 = 278;
const STRIP_BYTE_COUNTS: u16 = 279;
const PLANAR_CONFIG: u16 = 284;
const PREDICTOR: u16 = 317;
const TILE_WIDTH: u16 = 322;
const TILE_LENGTH: u16 = 323;
const TILE_OFFSETS: u16 = 324;
const TILE_BYTE_COUNTS: u16 = 325;
const SAMPLE_FORMAT: u16 = 339;
const MODEL_PIXEL_SCALE: u16 = 33550;
const MODEL_TIEPOINT: u16 = 33922;
const MODEL_TRANSFORMATION: u16 = 34264;
const GEO_KEY_DIRECTORY: u16 = 34735;
const GEO_DOUBLE_PARAMS: u16 = 34736;
const GEO_ASCII_PARAMS: u16 = 34737;
const GDAL_METADATA: u16 = 42112;
const GDAL_NODATA: u16 = 42113;

/// GeoKeys this crate reads.
pub mod key {
    pub const MODEL_TYPE: u16 = 1024;
    pub const RASTER_TYPE: u16 = 1025;
    pub const GEOGRAPHIC_TYPE: u16 = 2048;
    pub const GEOG_CITATION: u16 = 2049;
    pub const GEOG_GEODETIC_DATUM: u16 = 2050;
    pub const GEOG_ANGULAR_UNITS: u16 = 2054;
    pub const GEOG_ELLIPSOID: u16 = 2056;
    pub const GEOG_SEMI_MAJOR_AXIS: u16 = 2057;
    pub const GEOG_SEMI_MINOR_AXIS: u16 = 2058;
    pub const GEOG_INV_FLATTENING: u16 = 2059;
    pub const PROJECTED_CS_TYPE: u16 = 3072;
    pub const PROJ_COORD_TRANS: u16 = 3075;
    pub const PROJ_LINEAR_UNITS: u16 = 3076;
    pub const PROJ_NAT_ORIGIN_LONG: u16 = 3080;
    pub const PROJ_NAT_ORIGIN_LAT: u16 = 3081;
    pub const PROJ_FALSE_EASTING: u16 = 3082;
    pub const PROJ_FALSE_NORTHING: u16 = 3083;
    pub const PROJ_SCALE_AT_NAT_ORIGIN: u16 = 3092;
}

/// A field type's size in bytes.
fn type_size(t: u16) -> Option<u64> {
    Some(match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => return None,
    })
}

/// The file's byte order.
#[derive(Clone, Copy, Debug)]
struct Order {
    le: bool,
}

impl Order {
    fn uint(self, b: &[u8]) -> u64 {
        let mut v = 0u64;
        for i in 0..b.len() {
            v = v << 8 | b[if self.le { b.len() - 1 - i } else { i }] as u64;
        }
        v
    }

    fn f64(self, b: &[u8]) -> f64 {
        f64::from_bits(self.uint(&b[..8]))
    }
}

/// A directory entry: its type, count and value (inline, or at an offset).
#[derive(Clone, Debug)]
struct Entry {
    typ: u16,
    count: u64,
    at: Option<u64>,
    inline: Vec<u8>,
}

/// A tile or strip offsets or byte counts array: inline, or read entry by entry where it lies.
#[derive(Clone, Debug)]
enum Striles {
    Inline(Vec<u64>),
    At { off: u64, typ: u16, count: u64 },
}

/// One image of the file: the full-resolution one or an overview.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// A tile's size; for strips, the width by the rows a strip.
    pub block_w: u32,
    pub block_h: u32,
    tiled: bool,
    bits: u16,
    format: u16,
    spp: u16,
    planar: u16,
    compression: u16,
    predictor: u16,
    offsets: Striles,
    counts: Striles,
}

impl Image {
    fn blocks_across(&self) -> u32 {
        self.width.div_ceil(self.block_w)
    }
}

/// A GeoKey's value.
#[derive(Clone, Debug, PartialEq)]
pub enum GeoValue {
    Short(u16),
    Doubles(Vec<f64>),
    Ascii(String),
}

/// The GeoKeys of a file.
#[derive(Clone, Debug, Default)]
pub struct GeoKeys(pub BTreeMap<u16, GeoValue>);

impl GeoKeys {
    pub fn short(&self, k: u16) -> Option<u16> {
        match self.0.get(&k)? {
            GeoValue::Short(v) => Some(*v),
            _ => None,
        }
    }

    pub fn double(&self, k: u16) -> Option<f64> {
        match self.0.get(&k)? {
            GeoValue::Doubles(v) => v.first().copied(),
            _ => None,
        }
    }

    pub fn ascii(&self, k: u16) -> Option<&str> {
        match self.0.get(&k)? {
            GeoValue::Ascii(v) => Some(v),
            _ => None,
        }
    }
}

/// A decoded tile or strip's band-1 samples.
#[derive(Debug)]
enum Samples {
    U8(Vec<u8>),
    I8(Vec<i8>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    U32(Vec<u32>),
    I32(Vec<i32>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

/// A decoded tile or strip: `width` (the block's) by `height` samples of band 1.
#[derive(Debug)]
pub struct Block {
    pub width: usize,
    pub height: usize,
    data: Samples,
}

impl Block {
    /// The sample at column `x`, row `y`, as GDAL gives it in a 32-bit float buffer.
    pub fn f32_at(&self, x: usize, y: usize) -> f32 {
        let i = y * self.width + x;
        match &self.data {
            Samples::U8(v) => v[i] as f32,
            Samples::I8(v) => v[i] as f32,
            Samples::U16(v) => v[i] as f32,
            Samples::I16(v) => v[i] as f32,
            Samples::U32(v) => v[i] as f32,
            Samples::I32(v) => v[i] as f32,
            Samples::F32(v) => v[i],
            Samples::F64(v) => v[i] as f32,
        }
    }

    /// The samples, row by row, when they're 16-bit unsigned.
    pub fn u16s(&self) -> Option<&[u16]> {
        match &self.data {
            Samples::U16(v) => Some(v),
            _ => None,
        }
    }

    /// The samples, row by row, when they're 8-bit unsigned.
    pub fn u8s(&self) -> Option<&[u8]> {
        match &self.data {
            Samples::U8(v) => Some(v),
            _ => None,
        }
    }

    fn bytes(&self) -> usize {
        let n = self.width * self.height;
        match &self.data {
            Samples::U8(_) | Samples::I8(_) => n,
            Samples::U16(_) | Samples::I16(_) => 2 * n,
            Samples::U32(_) | Samples::I32(_) | Samples::F32(_) => 4 * n,
            Samples::F64(_) => 8 * n,
        }
    }
}

/// Decoded blocks kept for reuse, least recently used out first.
struct Lru {
    map: HashMap<(usize, u64), (Arc<Block>, u64)>,
    tick: u64,
    bytes: usize,
    cap: usize,
}

impl Lru {
    fn get(&mut self, k: (usize, u64)) -> Option<Arc<Block>> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(&k).map(|e| {
            e.1 = t;
            e.0.clone()
        })
    }

    fn put(&mut self, k: (usize, u64), b: Arc<Block>) {
        self.tick += 1;
        self.bytes += b.bytes();
        if let Some(old) = self.map.insert(k, (b, self.tick)) {
            self.bytes -= old.0.bytes();
        }
        while self.bytes > self.cap && self.map.len() > 1 {
            let oldest = *self.map.iter().min_by_key(|e| e.1 .1).unwrap().0;
            let (b, _) = self.map.remove(&oldest).unwrap();
            self.bytes -= b.bytes();
        }
    }
}

/// An open (Geo)TIFF.
pub struct Tiff {
    src: Arc<dyn RangeRead>,
    order: Order,
    /// The full-resolution image, then the overviews in the file's order (largest first).
    images: Vec<Image>,
    transform: [f64; 6],
    nodata: Option<f64>,
    keys: GeoKeys,
    /// GDAL's metadata items for the dataset (its default domain): name, value.
    metadata: Vec<(String, String)>,
    cache: Option<Mutex<Lru>>,
}

impl std::fmt::Debug for Tiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = &self.images[0];
        write!(f, "Tiff({}x{}, {} overviews)", m.width, m.height, self.images.len() - 1)
    }
}

/// A number as GDAL's `CPLAtofM` reads one: its leading numeric part, "nan" and "inf" included.
fn atof(s: &str) -> f64 {
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    let l = s.to_ascii_lowercase();
    let unsigned = l.trim_start_matches(['+', '-']);
    if unsigned.starts_with("nan") {
        return f64::NAN;
    }
    if unsigned.starts_with("inf") {
        return if l.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let mut end = s.len();
    while end > 0 {
        if let Ok(v) = s[..end].parse::<f64>() {
            return v;
        }
        end -= 1;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
    }
    0.0
}

impl Tiff {
    /// Opens the TIFF that `src` holds: its directories (each image's, the overviews' among them),
    /// georeferencing and nodata. Tiles are read as they're needed.
    pub fn open(src: Arc<dyn RangeRead>) -> Result<Tiff> {
        let len = src.len()?;
        ensure!(len >= 8, "not a TIFF ({len} bytes)");
        let head = src.read_at(0, len.min(16) as usize)?;
        let order = match &head[..2] {
            b"II" => Order { le: true },
            b"MM" => Order { le: false },
            _ => bail!("not a TIFF"),
        };
        let (big, mut ifd) = match order.uint(&head[2..4]) {
            42 => (false, order.uint(&head[4..8])),
            43 => {
                ensure!(head.len() >= 16 && order.uint(&head[4..6]) == 8, "a BigTIFF header that isn't whole");
                (true, order.uint(&head[8..16]))
            }
            v => bail!("TIFF version {v}"),
        };
        let mut dirs: Vec<BTreeMap<u16, Entry>> = Vec::new();
        while ifd != 0 {
            ensure!(dirs.len() < 256 && ifd < len, "TIFF directory at {ifd} past the end ({len} bytes)");
            let (cnt_sz, ent_sz, off_sz) = if big { (8usize, 20usize, 8usize) } else { (2, 12, 4) };
            let n = order.uint(&src.read_at(ifd, cnt_sz)?) as usize;
            ensure!(n < 4096, "TIFF directory of {n} entries");
            let b = src.read_at(ifd + cnt_sz as u64, n * ent_sz + off_sz).context("TIFF directory")?;
            let mut d = BTreeMap::new();
            for e in b[..n * ent_sz].chunks_exact(ent_sz) {
                let tag = order.uint(&e[..2]) as u16;
                let typ = order.uint(&e[2..4]) as u16;
                let count = order.uint(&e[4..4 + off_sz]);
                let Some(sz) = type_size(typ) else { continue };
                let value = &e[4 + off_sz..];
                let bytes = count.checked_mul(sz).context("TIFF entry size")?;
                let (at, inline) = if bytes <= off_sz as u64 { (None, value[..bytes as usize].to_vec()) } else { (Some(order.uint(value)), Vec::new()) };
                d.insert(tag, Entry { typ, count, at, inline });
            }
            dirs.push(d);
            ifd = order.uint(&b[n * ent_sz..]);
        }
        ensure!(!dirs.is_empty(), "a TIFF without images");
        let mut t = Tiff { src, order, images: Vec::new(), transform: [0.0, 1.0, 0.0, 0.0, 0.0, 1.0], nodata: None, keys: GeoKeys::default(), metadata: Vec::new(), cache: None };
        let main = t.image(&dirs[0]).context("TIFF image")?;
        t.images.push(main);
        for d in &dirs[1..] {
            let kind = t.uints(d, NEW_SUBFILE_TYPE)?.first().copied().unwrap_or(0);
            // Reduced-resolution images that aren't masks: the overviews.
            if kind & 1 == 0 || kind & 4 != 0 {
                continue;
            }
            let Ok(o) = t.image(d) else { continue };
            let m = &t.images[0];
            if o.width <= m.width && o.height <= m.height && o.spp == m.spp && o.bits == m.bits {
                t.images.push(o);
            }
        }
        t.geo(&dirs[0])?;
        Ok(t)
    }

    /// Keeps up to `bytes` of decoded tiles for reuse (for readers that come back to tiles).
    pub fn with_cache(mut self, bytes: usize) -> Self {
        self.cache = Some(Mutex::new(Lru { map: HashMap::new(), tick: 0, bytes: 0, cap: bytes }));
        self
    }

    /// The images: the full-resolution one first, then the overviews.
    pub fn images(&self) -> &[Image] {
        &self.images
    }

    /// Image `level`: 0 the full-resolution one, `k + 1` overview `k` (GDAL's OVERVIEW_LEVEL=k).
    pub fn level(&self, level: usize) -> Result<&Image> {
        self.images.get(level).with_context(|| format!("no overview {} (the file has {})", level as i64 - 1, self.images.len() - 1))
    }

    /// GDAL's geotransform of image `level`: the full-resolution one's, its scale times the sizes'
    /// ratio for an overview (GDAL's overview datasets).
    pub fn transform(&self, level: usize) -> Result<[f64; 6]> {
        let (m, o) = (&self.images[0], self.level(level)?);
        let mut gt = self.transform;
        let rx = m.width as f64 / o.width as f64;
        let ry = m.height as f64 / o.height as f64;
        gt[1] *= rx;
        gt[2] *= ry;
        gt[4] *= rx;
        gt[5] *= ry;
        Ok(gt)
    }

    /// GDAL's nodata value (its tag), when there's one.
    pub fn nodata(&self) -> Option<f64> {
        self.nodata
    }

    pub fn geo_keys(&self) -> &GeoKeys {
        &self.keys
    }

    /// GDAL's metadata item `name` for the dataset (as rasterio's `tags()` gives it), if any.
    pub fn metadata_item(&self, name: &str) -> Option<&str> {
        self.metadata.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn entry_bytes(&self, e: &Entry) -> Result<Vec<u8>> {
        match e.at {
            None => Ok(e.inline.clone()),
            Some(off) => {
                let n = e.count * type_size(e.typ).unwrap_or(1);
                ensure!(n <= 64 << 20, "a TIFF value of {n} bytes");
                Ok(self.src.read_at(off, n as usize)?)
            }
        }
    }

    /// A tag's values as unsigned integers (empty when it's absent).
    fn uints(&self, d: &BTreeMap<u16, Entry>, tag: u16) -> Result<Vec<u64>> {
        let Some(e) = d.get(&tag) else { return Ok(Vec::new()) };
        let sz = type_size(e.typ).unwrap() as usize;
        ensure!(matches!(e.typ, 1 | 3 | 4 | 16), "TIFF tag {tag}: type {}", e.typ);
        Ok(self.entry_bytes(e)?.chunks_exact(sz).map(|c| self.order.uint(c)).collect())
    }

    fn doubles(&self, d: &BTreeMap<u16, Entry>, tag: u16) -> Result<Vec<f64>> {
        let Some(e) = d.get(&tag) else { return Ok(Vec::new()) };
        ensure!(e.typ == 12, "TIFF tag {tag}: type {}", e.typ);
        Ok(self.entry_bytes(e)?.as_chunks::<8>().0.iter().map(|c| self.order.f64(c)).collect())
    }

    fn ascii(&self, d: &BTreeMap<u16, Entry>, tag: u16) -> Result<Option<String>> {
        let Some(e) = d.get(&tag) else { return Ok(None) };
        let b = self.entry_bytes(e)?;
        Ok(Some(String::from_utf8_lossy(&b).trim_end_matches('\0').to_string()))
    }

    fn striles(&self, d: &BTreeMap<u16, Entry>, tag: u16) -> Result<Striles> {
        let e = d.get(&tag).with_context(|| format!("TIFF tag {tag} missing"))?;
        ensure!(matches!(e.typ, 3 | 4 | 16), "TIFF tag {tag}: type {}", e.typ);
        Ok(match e.at {
            None => Striles::Inline(self.uints(d, tag)?),
            Some(off) => Striles::At { off, typ: e.typ, count: e.count },
        })
    }

    fn image(&self, d: &BTreeMap<u16, Entry>) -> Result<Image> {
        let one = |tag: u16, default: u64| -> Result<u64> { Ok(self.uints(d, tag)?.first().copied().unwrap_or(default)) };
        let width = one(IMAGE_WIDTH, 0)? as u32;
        let height = one(IMAGE_LENGTH, 0)? as u32;
        ensure!(width > 0 && height > 0, "TIFF image of {width}x{height}");
        let bits = one(BITS_PER_SAMPLE, 1)? as u16;
        let format = one(SAMPLE_FORMAT, 1)? as u16;
        let spp = one(SAMPLES_PER_PIXEL, 1)? as u16;
        let planar = one(PLANAR_CONFIG, 1)? as u16;
        let compression = one(COMPRESSION, 1)? as u16;
        let predictor = one(PREDICTOR, 1)? as u16;
        ensure!(matches!(bits, 8 | 16 | 32 | 64), "{bits}-bit samples");
        ensure!(matches!((format, bits), (1 | 2, _) | (3, 32 | 64)), "sample format {format} of {bits} bits");
        ensure!(matches!(compression, 1 | 5 | 8 | 32946 | 50000), "TIFF compression {compression}");
        ensure!(matches!(predictor, 1..=3), "TIFF predictor {predictor}");
        ensure!(predictor != 3 || format == 3, "the floating-point predictor on integers");
        ensure!(spp >= 1 && matches!(planar, 1 | 2), "{spp} samples a pixel, planar configuration {planar}");
        let tiled = d.contains_key(&TILE_WIDTH);
        let (block_w, block_h, offsets, counts) = if tiled {
            let (tw, th) = (one(TILE_WIDTH, 0)? as u32, one(TILE_LENGTH, 0)? as u32);
            ensure!(tw > 0 && th > 0, "tiles of {tw}x{th}");
            (tw, th, self.striles(d, TILE_OFFSETS)?, self.striles(d, TILE_BYTE_COUNTS)?)
        } else {
            let rps = one(ROWS_PER_STRIP, u32::MAX as u64)?.clamp(1, height as u64) as u32;
            (width, rps, self.striles(d, STRIP_OFFSETS)?, self.striles(d, STRIP_BYTE_COUNTS)?)
        };
        Ok(Image { width, height, block_w, block_h, tiled, bits, format, spp, planar, compression, predictor, offsets, counts })
    }

    fn geo(&mut self, d: &BTreeMap<u16, Entry>) -> Result<()> {
        if d.contains_key(&GEO_KEY_DIRECTORY) {
            let k = self.uints(d, GEO_KEY_DIRECTORY)?;
            let dbl = self.doubles(d, GEO_DOUBLE_PARAMS).unwrap_or_default();
            let asc = self.ascii(d, GEO_ASCII_PARAMS)?.unwrap_or_default();
            if k.len() >= 4 {
                let n = k[3] as usize;
                for e in k[4..].as_chunks::<4>().0.iter().take(n) {
                    let (id, loc, count, v) = (e[0] as u16, e[1] as u16, e[2] as usize, e[3] as usize);
                    let val = match loc {
                        0 => GeoValue::Short(v as u16),
                        GEO_DOUBLE_PARAMS => GeoValue::Doubles(dbl.get(v..v + count).map(<[f64]>::to_vec).unwrap_or_default()),
                        GEO_ASCII_PARAMS => GeoValue::Ascii(asc.get(v..v + count).unwrap_or("").trim_end_matches(['|', '\0']).to_string()),
                        GEO_KEY_DIRECTORY => GeoValue::Short(k.get(v).copied().unwrap_or(0) as u16),
                        _ => continue,
                    };
                    self.keys.0.insert(id, val);
                }
            }
        }
        let point = self.keys.short(key::RASTER_TYPE) == Some(2);
        let scale = self.doubles(d, MODEL_PIXEL_SCALE)?;
        let tie = self.doubles(d, MODEL_TIEPOINT)?;
        let matrix = self.doubles(d, MODEL_TRANSFORMATION)?;
        let mut gt = [0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let mut valid = false;
        if scale.len() >= 2 && scale[0] != 0.0 && scale[1] != 0.0 {
            gt = [0.0; 6];
            gt[1] = scale[0];
            // (GDAL takes a negative scale as meant north-up.)
            gt[5] = if scale[1] < 0.0 { scale[1] } else { -scale[1] };
            if tie.len() >= 6 {
                // (GDAL's build fuses these multiply-adds.)
                gt[0] = (-tie[0]).mul_add(gt[1], tie[3]);
                gt[3] = (-tie[1]).mul_add(gt[5], tie[4]);
                valid = true;
            }
        } else if matrix.len() == 16 {
            gt = [matrix[3], matrix[0], matrix[1], matrix[7], matrix[4], matrix[5]];
            valid = true;
        }
        if valid {
            if point {
                gt[0] -= gt[1] * 0.5 + gt[2] * 0.5;
                gt[3] -= gt[4] * 0.5 + gt[5] * 0.5;
            }
            self.transform = gt;
        } else {
            self.transform = [0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        }
        if let Some(x) = self.ascii(d, GDAL_METADATA)? {
            self.metadata = metadata_items(&x);
        }
        if let Some(s) = self.ascii(d, GDAL_NODATA)?.filter(|s| !s.is_empty()) {
            let mut v = atof(&s);
            let m = &self.images[0];
            if m.bits == 32 && m.format == 3 {
                // GDALAdjustNoDataCloseToFloatMax.
                let max = f32::MAX as f64;
                if (v + max).abs() < 1e-10 * max {
                    v = -max;
                } else if (v - max).abs() < 1e-10 * max {
                    v = max;
                }
            }
            self.nodata = Some(v);
        }
        Ok(())
    }

    fn strile(&self, s: &Striles, i: u64) -> Result<u64> {
        match s {
            Striles::Inline(v) => v.get(i as usize).copied().context("tile index past the TIFF's list"),
            Striles::At { off, typ, count } => {
                ensure!(i < *count, "tile index {i} past the TIFF's {count}");
                let sz = type_size(*typ).unwrap();
                Ok(self.order.uint(&self.src.read_at(off + i * sz, sz as usize)?))
            }
        }
    }

    /// Block (`bx`, `by`) of image `level`, decoded (from the cache when there's one).
    pub fn block(&self, level: usize, bx: u32, by: u32) -> Result<Arc<Block>> {
        let img = self.level(level)?;
        let index = by as u64 * img.blocks_across() as u64 + bx as u64;
        if let Some(c) = &self.cache {
            if let Some(b) = c.lock().unwrap().get((level, index)) {
                return Ok(b);
            }
        }
        let b = Arc::new(self.decode(img, index, by).with_context(|| format!("TIFF block {bx},{by} of image {level}"))?);
        if let Some(c) = &self.cache {
            c.lock().unwrap().put((level, index), b.clone());
        }
        Ok(b)
    }

    fn decode(&self, img: &Image, index: u64, by: u32) -> Result<Block> {
        let rows = if img.tiled { img.block_h } else { img.block_h.min(img.height - by * img.block_h) } as usize;
        let width = img.block_w as usize;
        let spb = if img.planar == 2 { 1 } else { img.spp as usize };
        let bps = img.bits as usize / 8;
        let row_bytes = width * spb * bps;
        let expect = rows * row_bytes;
        let off = self.strile(&img.offsets, index)?;
        let n = self.strile(&img.counts, index)?;
        let mut bytes = if off == 0 || n == 0 {
            // A sparse block: GDAL fills it with the nodata value (or 0).
            return Ok(Block { width, height: rows, data: filled(img, width * rows, self.nodata.unwrap_or(0.0)) });
        } else {
            ensure!(n <= 1 << 30, "a TIFF block of {n} bytes");
            let raw = self.src.read_at(off, n as usize)?;
            decompress(img.compression, &raw, expect)?
        };
        if img.predictor == 3 {
            let mut tmp = Vec::with_capacity(row_bytes);
            for row in bytes.chunks_exact_mut(row_bytes) {
                fp_acc(row, bps, spb, &mut tmp);
            }
        } else {
            if !self.order.le && bps > 1 {
                for s in bytes.chunks_exact_mut(bps) {
                    s.reverse();
                }
            }
            if img.predictor == 2 {
                for row in bytes.chunks_exact_mut(row_bytes) {
                    hor_acc(row, bps, spb);
                }
            }
        }
        Ok(Block { width, height: rows, data: samples(img, &bytes, spb) })
    }

    /// Image `level`'s pixels `x0..x0 + w` by `y0..y0 + h`, band 1, as 32-bit floats (as GDAL
    /// reads a window into a Float32 buffer), row by row.
    pub fn read_window(&self, level: usize, x0: u32, y0: u32, w: u32, h: u32) -> Result<Vec<f32>> {
        let img = self.level(level)?;
        ensure!(w > 0 && h > 0 && x0 as u64 + w as u64 <= img.width as u64 && y0 as u64 + h as u64 <= img.height as u64, "window {x0},{y0} {w}x{h} outside the {}x{} image", img.width, img.height);
        let (bw, bh) = (img.block_w, img.block_h);
        let mut out = vec![0f32; w as usize * h as usize];
        for by in y0 / bh..=(y0 + h - 1) / bh {
            for bx in x0 / bw..=(x0 + w - 1) / bw {
                let b = self.block(level, bx, by)?;
                let (ox, oy) = (bx * bw, by * bh);
                let (xs, xe) = (x0.max(ox), (x0 + w).min(ox + bw));
                let (ys, ye) = (y0.max(oy), (y0 + h).min(oy + b.height as u32));
                for y in ys..ye {
                    let o = (y - y0) as usize * w as usize;
                    for x in xs..xe {
                        out[o + (x - x0) as usize] = b.f32_at((x - ox) as usize, (y - oy) as usize);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Image `level`'s pixel (`x`, `y`), band 1, as a 32-bit float.
    pub fn pixel(&self, level: usize, x: u32, y: u32) -> Result<f32> {
        let img = self.level(level)?;
        let (bx, by) = (x / img.block_w, y / img.block_h);
        let b = self.block(level, bx, by)?;
        Ok(b.f32_at((x - bx * img.block_w) as usize, (y - by * img.block_h) as usize))
    }
}

/// The dataset's items of GDAL's metadata XML (`<Item name="…">…</Item>`, those of no domain and no
/// band), their text unescaped.
fn metadata_items(xml: &str) -> Vec<(String, String)> {
    let unescape = |s: &str| s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<Item ") {
        rest = &rest[i + 6..];
        let Some(close) = rest.find('>') else { break };
        let (attrs, body) = match rest[..close].strip_suffix('/') {
            Some(a) => {
                rest = &rest[close + 1..];
                (a, "")
            }
            None => {
                let Some(end) = rest.find("</Item>") else { break };
                let (a, b) = (&rest[..close], rest.get(close + 1..end).unwrap_or(""));
                rest = &rest[end + 7..];
                (a, b)
            }
        };
        let attr = |k: &str| attrs.split_once(&format!(" {k}=\"")).or_else(|| attrs.strip_prefix(&format!("{k}=\"")).map(|v| ("", v))).and_then(|(_, v)| v.split_once('"')).map(|(v, _)| v);
        if attr("domain").is_some() || attr("sample").is_some() {
            continue;
        }
        if let Some(name) = attr("name") {
            out.push((unescape(name), unescape(body)));
        }
    }
    out
}

/// A block filled with `v` (in the image's sample type).
fn filled(img: &Image, n: usize, v: f64) -> Samples {
    match (img.format, img.bits) {
        (1, 8) => Samples::U8(vec![v as u8; n]),
        (2, 8) => Samples::I8(vec![v as i8; n]),
        (1, 16) => Samples::U16(vec![v as u16; n]),
        (2, 16) => Samples::I16(vec![v as i16; n]),
        (1, 32) => Samples::U32(vec![v as u32; n]),
        (2, 32) => Samples::I32(vec![v as i32; n]),
        (3, 32) => Samples::F32(vec![v as f32; n]),
        _ => Samples::F64(vec![v; n]),
    }
}

/// Band 1 of decoded native-order bytes (`spb` samples a pixel).
fn samples(img: &Image, b: &[u8], spb: usize) -> Samples {
    let bps = img.bits as usize / 8;
    let px = b.chunks_exact(bps * spb).map(|c| &c[..bps]);
    match (img.format, img.bits) {
        (1, 8) => Samples::U8(px.map(|c| c[0]).collect()),
        (2, 8) => Samples::I8(px.map(|c| c[0] as i8).collect()),
        (1, 16) => Samples::U16(px.map(|c| u16::from_le_bytes([c[0], c[1]])).collect()),
        (2, 16) => Samples::I16(px.map(|c| i16::from_le_bytes([c[0], c[1]])).collect()),
        (1, 32) => Samples::U32(px.map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()),
        (2, 32) => Samples::I32(px.map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect()),
        (3, 32) => Samples::F32(px.map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()),
        (3, 64) => Samples::F64(px.map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect()),
        (1, 64) => Samples::F64(px.map(|c| u64::from_le_bytes(c.try_into().unwrap()) as f64).collect()),
        _ => Samples::F64(px.map(|c| i64::from_le_bytes(c.try_into().unwrap()) as f64).collect()),
    }
}

/// A block's bytes decompressed: at least `expect` of them (more are cut off).
fn decompress(compression: u16, raw: &[u8], expect: usize) -> Result<Vec<u8>> {
    let mut out = match compression {
        1 => raw.to_vec(),
        5 => {
            // libtiff's old-style LZW (low bit first, no early change) starts 0x00 0x01.
            let mut dec = if raw.len() >= 2 && raw[0] == 0 && raw[1] & 1 == 1 {
                weezl::decode::Decoder::new(weezl::BitOrder::Lsb, 8)
            } else {
                weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
            };
            let mut out = Vec::with_capacity(expect);
            let r = dec.into_vec(&mut out).decode(raw);
            if let Err(e) = r.status {
                bail!("LZW: {e:?}");
            }
            out
        }
        8 | 32946 => {
            let mut d = flate2::Decompress::new(true);
            let mut out = Vec::with_capacity(expect);
            d.decompress_vec(raw, &mut out, flate2::FlushDecompress::Finish).context("Deflate")?;
            out
        }
        50000 => zstd::bulk::decompress(raw, expect).context("Zstandard")?,
        c => bail!("TIFF compression {c}"),
    };
    ensure!(out.len() >= expect, "a block decodes to {} of its {expect} bytes", out.len());
    out.truncate(expect);
    Ok(out)
}

/// Undoes horizontal differencing (predictor 2) along a row of native-order samples, `stride`
/// samples a pixel.
fn hor_acc(row: &mut [u8], bps: usize, stride: usize) {
    let n = row.len() / bps;
    match bps {
        1 => {
            for i in stride..n {
                row[i] = row[i].wrapping_add(row[i - stride]);
            }
        }
        2 => {
            for i in stride..n {
                let a = u16::from_le_bytes([row[2 * (i - stride)], row[2 * (i - stride) + 1]]);
                let b = u16::from_le_bytes([row[2 * i], row[2 * i + 1]]);
                row[2 * i..2 * i + 2].copy_from_slice(&a.wrapping_add(b).to_le_bytes());
            }
        }
        4 => {
            for i in stride..n {
                let a = u32::from_le_bytes(row[4 * (i - stride)..4 * (i - stride) + 4].try_into().unwrap());
                let b = u32::from_le_bytes(row[4 * i..4 * i + 4].try_into().unwrap());
                row[4 * i..4 * i + 4].copy_from_slice(&a.wrapping_add(b).to_le_bytes());
            }
        }
        _ => {
            for i in stride..n {
                let a = u64::from_le_bytes(row[8 * (i - stride)..8 * (i - stride) + 8].try_into().unwrap());
                let b = u64::from_le_bytes(row[8 * i..8 * i + 8].try_into().unwrap());
                row[8 * i..8 * i + 8].copy_from_slice(&a.wrapping_add(b).to_le_bytes());
            }
        }
    }
}

/// Undoes the floating-point predictor (3) on a row, as libtiff's `fpAcc` does: bytes summed
/// along the row, then each sample's bytes gathered from their planes (most significant first)
/// into little-endian order, whatever the file's.
fn fp_acc(row: &mut [u8], bps: usize, stride: usize, tmp: &mut Vec<u8>) {
    for i in stride..row.len() {
        row[i] = row[i].wrapping_add(row[i - stride]);
    }
    let wc = row.len() / bps;
    tmp.clear();
    tmp.extend_from_slice(row);
    for count in 0..wc {
        for byte in 0..bps {
            row[bps * count + byte] = tmp[(bps - byte - 1) * wc + count];
        }
    }
}

/// The floating-point predictor's encoding of a row of little-endian samples (`fp_acc` undoes it).
fn fp_diff(row: &mut [u8], bps: usize, stride: usize, tmp: &mut Vec<u8>) {
    let wc = row.len() / bps;
    tmp.clear();
    tmp.resize(row.len(), 0);
    for count in 0..wc {
        for byte in 0..bps {
            tmp[(bps - byte - 1) * wc + count] = row[bps * count + byte];
        }
    }
    for i in (stride..row.len()).rev() {
        tmp[i] = tmp[i].wrapping_sub(tmp[i - stride]);
    }
    row.copy_from_slice(tmp);
}

/// GeoKeys for a geographic CRS by its EPSG code (as GDAL writes them for WGS 84).
fn geographic_keys(epsg: u16) -> (Vec<u16>, Vec<f64>, String) {
    let mut keys: Vec<[u16; 4]> = vec![[key::MODEL_TYPE, 0, 1, 2], [key::RASTER_TYPE, 0, 1, 1], [key::GEOGRAPHIC_TYPE, 0, 1, epsg]];
    let mut dbl = Vec::new();
    let mut asc = String::new();
    if epsg == 4326 {
        asc.push_str("WGS 84|");
        keys.push([key::GEOG_CITATION, GEO_ASCII_PARAMS, 7, 0]);
        keys.push([key::GEOG_ANGULAR_UNITS, 0, 1, 9102]);
        keys.push([key::GEOG_SEMI_MAJOR_AXIS, GEO_DOUBLE_PARAMS, 1, 0]);
        keys.push([key::GEOG_INV_FLATTENING, GEO_DOUBLE_PARAMS, 1, 1]);
        dbl = vec![6378137.0, 298.257223563];
    } else {
        keys.push([key::GEOG_ANGULAR_UNITS, 0, 1, 9102]);
    }
    let mut dir = vec![1u16, 1, 0, keys.len() as u16];
    for k in keys {
        dir.extend_from_slice(&k);
    }
    (dir, dbl, asc)
}

/// GDAL's nodata tag text for `v`.
fn nodata_text(v: f64) -> String {
    if v.is_nan() {
        "nan".into()
    } else if v.is_infinite() {
        if v > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        format!("{v}")
    }
}

/// A one-band 32-bit float GeoTIFF of `width` by `height` (`data`, row by row): little-endian,
/// `tile`-pixel square tiles (edges filled with the nodata value, or 0), Deflate with the
/// floating-point predictor; north-up geotransform `gt` in geographic CRS `epsg`, and GDAL's
/// nodata tag.
pub fn write_f32(width: u32, height: u32, data: &[f32], tile: u32, gt: [f64; 6], epsg: u16, nodata: Option<f64>) -> Result<Vec<u8>> {
    use rayon::prelude::*;
    use std::io::Write;
    ensure!(data.len() == width as usize * height as usize && width > 0 && height > 0 && tile > 0, "a {width}x{height} raster of {} values", data.len());
    ensure!(gt[2] == 0.0 && gt[4] == 0.0 && gt[5] < 0.0, "a rotated or south-up geotransform");
    let (across, down) = (width.div_ceil(tile), height.div_ceil(tile));
    let fill = nodata.unwrap_or(0.0) as f32;
    let tiles: Vec<Vec<u8>> = (0..across * down)
        .into_par_iter()
        .map(|t| -> Result<Vec<u8>> {
            let (tx, ty) = (t % across, t / across);
            let ts = tile as usize;
            let mut b = vec![0u8; ts * ts * 4];
            let mut tmp = Vec::new();
            for r in 0..ts {
                let row = &mut b[r * ts * 4..(r + 1) * ts * 4];
                let y = ty as usize * ts + r;
                for c in 0..ts {
                    let x = tx as usize * ts + c;
                    let v = if x < width as usize && y < height as usize { data[y * width as usize + x] } else { fill };
                    row[c * 4..c * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
                fp_diff(row, 4, 1, &mut tmp);
            }
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
            z.write_all(&b)?;
            Ok(z.finish()?)
        })
        .collect::<Result<_>>()?;
    let (gk, gd, ga) = geographic_keys(epsg);
    let nd = nodata.map(|v| format!("{}\0", nodata_text(v)));
    // Entries: (tag, type, count, value bytes).
    let n = tiles.len();
    let mut entries: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (IMAGE_WIDTH, 4, 1, width.to_le_bytes().to_vec()),
        (IMAGE_LENGTH, 4, 1, height.to_le_bytes().to_vec()),
        (BITS_PER_SAMPLE, 3, 1, 32u16.to_le_bytes().to_vec()),
        (COMPRESSION, 3, 1, 8u16.to_le_bytes().to_vec()),
        (262, 3, 1, 1u16.to_le_bytes().to_vec()),
        (SAMPLES_PER_PIXEL, 3, 1, 1u16.to_le_bytes().to_vec()),
        (PLANAR_CONFIG, 3, 1, 1u16.to_le_bytes().to_vec()),
        (PREDICTOR, 3, 1, 3u16.to_le_bytes().to_vec()),
        (TILE_WIDTH, 4, 1, tile.to_le_bytes().to_vec()),
        (TILE_LENGTH, 4, 1, tile.to_le_bytes().to_vec()),
        (TILE_OFFSETS, 4, n as u32, vec![0; 4 * n]),
        (TILE_BYTE_COUNTS, 4, n as u32, tiles.iter().flat_map(|t| (t.len() as u32).to_le_bytes()).collect()),
        (SAMPLE_FORMAT, 3, 1, 3u16.to_le_bytes().to_vec()),
        (MODEL_PIXEL_SCALE, 12, 3, [gt[1], -gt[5], 0.0].iter().flat_map(|v| v.to_le_bytes()).collect()),
        (MODEL_TIEPOINT, 12, 6, [0.0, 0.0, 0.0, gt[0], gt[3], 0.0].iter().flat_map(|v| v.to_le_bytes()).collect()),
        (GEO_KEY_DIRECTORY, 3, gk.len() as u32, gk.iter().flat_map(|v| v.to_le_bytes()).collect()),
    ];
    if !gd.is_empty() {
        entries.push((GEO_DOUBLE_PARAMS, 12, gd.len() as u32, gd.iter().flat_map(|v| v.to_le_bytes()).collect()));
    }
    if !ga.is_empty() {
        let mut a = ga.into_bytes();
        a.push(0);
        entries.push((GEO_ASCII_PARAMS, 2, a.len() as u32, a));
    }
    if let Some(nd) = nd {
        entries.push((GDAL_NODATA, 2, nd.len() as u32, nd.into_bytes()));
    }
    // Header, directory, the values that don't fit their entries, then the tiles.
    let ifd_len = 2 + 12 * entries.len() + 4;
    let mut extra_len = 0usize;
    for e in &entries {
        if e.3.len() > 4 {
            extra_len += e.3.len().next_multiple_of(2);
        }
    }
    let data_start = 8 + ifd_len + extra_len;
    let total = data_start + tiles.iter().map(Vec::len).sum::<usize>();
    ensure!(total < u32::MAX as usize, "a GeoTIFF of {total} bytes needs BigTIFF");
    let mut offs = Vec::with_capacity(4 * n);
    let mut at = data_start as u32;
    for t in &tiles {
        offs.extend_from_slice(&at.to_le_bytes());
        at += t.len() as u32;
    }
    entries[10].3 = offs;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"II*\0");
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut extra: Vec<u8> = Vec::with_capacity(extra_len);
    for (tag, typ, count, v) in &entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&typ.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        if v.len() <= 4 {
            let mut b = [0u8; 4];
            b[..v.len()].copy_from_slice(v);
            out.extend_from_slice(&b);
        } else {
            out.extend_from_slice(&((8 + ifd_len + extra.len()) as u32).to_le_bytes());
            extra.extend_from_slice(v);
            if extra.len() % 2 == 1 {
                extra.push(0);
            }
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&extra);
    debug_assert_eq!(out.len(), data_start);
    for t in &tiles {
        out.extend_from_slice(t);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod testtiff {
    //! TIFFs of every layout the reader takes, for tests.
    use super::*;
    use std::io::Write;

    #[derive(Clone, Copy, Debug)]
    pub struct Opts {
        pub le: bool,
        pub big: bool,
        /// Tile size; None for strips of `rows` rows.
        pub tile: Option<u32>,
        pub rows: u32,
        pub compression: u16,
        pub predictor: u16,
        /// (format, bits): (3, 32) f32, (2, 16) i16, (1, 16) u16, (1, 8) u8.
        pub kind: (u16, u16),
        pub spp: u16,
        /// The GeoKey directory (header included), if any.
        pub keys: &'static [u16],
    }

    impl Default for Opts {
        fn default() -> Self {
            Opts { le: true, big: false, tile: Some(16), rows: 5, compression: 1, predictor: 1, kind: (3, 32), spp: 1, keys: &[] }
        }
    }

    /// Sample `i` of a test raster as `kind`'s bytes (native), and the f32 GDAL would give.
    pub fn value(kind: (u16, u16), i: usize, band: usize) -> f64 {
        let v = (i * 7919 + band * 104729) % 65521;
        match kind {
            (3, _) => (v as f64 - 30000.0) * 0.37,
            (2, 16) => v as f64 - 32760.0,
            (1, 16) => v as f64,
            _ => (v % 251) as f64,
        }
    }

    fn sample_bytes(kind: (u16, u16), v: f64) -> Vec<u8> {
        match kind {
            (3, 32) => (v as f32).to_le_bytes().to_vec(),
            (3, 64) => v.to_le_bytes().to_vec(),
            (2, 16) => (v as i16).to_le_bytes().to_vec(),
            (1, 16) => (v as u16).to_le_bytes().to_vec(),
            _ => vec![v as u8],
        }
    }

    /// A `w` by `h` test raster as a TIFF in layout `o`, and its band-1 values as f32.
    pub fn make(w: u32, h: u32, o: Opts, gt: [f64; 6], nodata: Option<&str>) -> (Vec<u8>, Vec<f32>) {
        let (b, mut v) = make_images(&[(w, h)], o, gt, nodata, &|_, x, y, s| value(o.kind, (y * w + x) as usize, s));
        (b, v.remove(0))
    }

    /// A TIFF of images of `sizes` (the first full-resolution, the others overviews of it), in
    /// layout `o`, sample `s` of pixel (`x`, `y`) of image `k` being `val(k, x, y, s)`; and each
    /// image's band-1 values as f32.
    pub fn make_images(sizes: &[(u32, u32)], o: Opts, gt: [f64; 6], nodata: Option<&str>, val: &dyn Fn(usize, u32, u32, usize) -> f64) -> (Vec<u8>, Vec<Vec<f32>>) {
        let bps = o.kind.1 as usize / 8;
        let spp = o.spp as usize;
        let ord = |v: u64, n: usize| -> Vec<u8> {
            let b = v.to_le_bytes();
            if o.le { b[..n].to_vec() } else { b[..n].iter().rev().copied().collect() }
        };
        let dbl = |v: f64| -> Vec<u8> { ord(v.to_bits(), 8) };
        let (cnt_sz, ent_sz, off_sz) = if o.big { (8usize, 20usize, 8usize) } else { (2, 12, 4) };
        let header = if o.big { 16 } else { 8 };
        // Each image's blocks and directory entries (tag, type, count, value).
        type Image = (Vec<Vec<u8>>, Vec<(u16, u16, u64, Vec<u8>)>);
        let mut images: Vec<Image> = Vec::new();
        let mut values: Vec<Vec<f32>> = Vec::new();
        for (k, &(w, h)) in sizes.iter().enumerate() {
            let (bw, bh) = match o.tile {
                Some(t) => (t, t),
                None => (w, o.rows),
            };
            let (across, down) = (w.div_ceil(bw), h.div_ceil(bh));
            let mut blocks: Vec<Vec<u8>> = Vec::new();
            for by in 0..down {
                for bx in 0..across {
                    let rows = if o.tile.is_some() { bh } else { bh.min(h - by * bh) };
                    let mut b = Vec::new();
                    for r in 0..rows {
                        let mut row = Vec::new();
                        for c in 0..bw {
                            let (x, y) = (bx * bw + c, by * bh + r);
                            for s in 0..spp {
                                let v = if x < w && y < h { val(k, x, y, s) } else { 0.0 };
                                row.extend(sample_bytes(o.kind, v));
                            }
                        }
                        match o.predictor {
                            2 => {
                                let n = row.len() / bps;
                                for i in (spp..n).rev() {
                                    match bps {
                                        1 => row[i] = row[i].wrapping_sub(row[i - spp]),
                                        2 => {
                                            let a = u16::from_le_bytes([row[2 * (i - spp)], row[2 * (i - spp) + 1]]);
                                            let b = u16::from_le_bytes([row[2 * i], row[2 * i + 1]]);
                                            row[2 * i..2 * i + 2].copy_from_slice(&b.wrapping_sub(a).to_le_bytes());
                                        }
                                        _ => {
                                            let a = u32::from_le_bytes(row[4 * (i - spp)..4 * (i - spp) + 4].try_into().unwrap());
                                            let b = u32::from_le_bytes(row[4 * i..4 * i + 4].try_into().unwrap());
                                            row[4 * i..4 * i + 4].copy_from_slice(&b.wrapping_sub(a).to_le_bytes());
                                        }
                                    }
                                }
                            }
                            3 => {
                                let mut tmp = Vec::new();
                                fp_diff(&mut row, bps, spp, &mut tmp);
                            }
                            _ => {}
                        }
                        if !o.le && o.predictor != 3 && bps > 1 {
                            for s in row.chunks_exact_mut(bps) {
                                s.reverse();
                            }
                        }
                        b.extend(row);
                    }
                    let b = match o.compression {
                        5 => weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8).encode(&b).unwrap(),
                        8 => {
                            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                            z.write_all(&b).unwrap();
                            z.finish().unwrap()
                        }
                        50000 => zstd::bulk::compress(&b, 3).unwrap(),
                        _ => b,
                    };
                    blocks.push(b);
                }
            }
            let nb = blocks.len() as u64;
            let mut e: Vec<(u16, u16, u64, Vec<u64>)> = vec![
                (IMAGE_WIDTH, 4, 1, vec![w as u64]),
                (IMAGE_LENGTH, 3, 1, vec![h as u64]),
                (BITS_PER_SAMPLE, 3, o.spp as u64, vec![o.kind.1 as u64; spp]),
                (COMPRESSION, 3, 1, vec![o.compression as u64]),
                (SAMPLES_PER_PIXEL, 3, 1, vec![o.spp as u64]),
                (PREDICTOR, 3, 1, vec![o.predictor as u64]),
                (SAMPLE_FORMAT, 3, o.spp as u64, vec![o.kind.0 as u64; spp]),
            ];
            if k > 0 {
                e.push((NEW_SUBFILE_TYPE, 4, 1, vec![1]));
            }
            let off_type = if o.big { 16 } else { 4 };
            match o.tile {
                Some(t) => {
                    e.push((TILE_WIDTH, 3, 1, vec![t as u64]));
                    e.push((TILE_LENGTH, 3, 1, vec![t as u64]));
                    e.push((TILE_OFFSETS, off_type, nb, vec![0; nb as usize]));
                    e.push((TILE_BYTE_COUNTS, 4, nb, blocks.iter().map(|b| b.len() as u64).collect()));
                }
                None => {
                    e.push((ROWS_PER_STRIP, 3, 1, vec![o.rows as u64]));
                    e.push((STRIP_OFFSETS, off_type, nb, vec![0; nb as usize]));
                    e.push((STRIP_BYTE_COUNTS, 4, nb, blocks.iter().map(|b| b.len() as u64).collect()));
                }
            }
            let mut all: Vec<(u16, u16, u64, Vec<u8>)> = e
                .into_iter()
                .map(|(t, ty, c, v)| {
                    let sz = type_size(ty).unwrap() as usize;
                    (t, ty, c, v.iter().flat_map(|&x| ord(x, sz)).collect())
                })
                .collect();
            // Georeferencing on the full-resolution image, nodata on each.
            if k == 0 && !o.keys.is_empty() {
                all.push((GEO_KEY_DIRECTORY, 3, o.keys.len() as u64, o.keys.iter().flat_map(|&v| ord(v as u64, 2)).collect()));
            }
            if k == 0 {
                all.push((MODEL_PIXEL_SCALE, 12, 3, [gt[1], -gt[5], 0.0].iter().flat_map(|&v| dbl(v)).collect()));
                all.push((MODEL_TIEPOINT, 12, 6, [0.0, 0.0, 0.0, gt[0], gt[3], 0.0].iter().flat_map(|&v| dbl(v)).collect()));
            }
            if let Some(nd) = nodata {
                let mut a = nd.as_bytes().to_vec();
                a.push(0);
                all.push((GDAL_NODATA, 2, a.len() as u64, a));
            }
            all.sort_by_key(|x| x.0);
            values.push((0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| sample_value(o.kind, val(k, x, y, 0))).collect());
            images.push((blocks, all));
        }
        // The directories (each followed by its values), then the blocks.
        let mut ifd_at = Vec::new();
        let mut pos = header;
        for (_, ents) in &images {
            ifd_at.push(pos);
            pos += cnt_sz + ent_sz * ents.len() + off_sz + ents.iter().filter(|x| x.3.len() > off_sz).map(|x| x.3.len()).sum::<usize>();
        }
        let mut at = pos as u64;
        for (blocks, ents) in images.iter_mut() {
            let offs: Vec<u64> = blocks
                .iter()
                .map(|b| {
                    let o = at;
                    at += b.len() as u64;
                    o
                })
                .collect();
            let oi = ents.iter().position(|x| x.0 == TILE_OFFSETS || x.0 == STRIP_OFFSETS).unwrap();
            let sz = type_size(ents[oi].1).unwrap() as usize;
            ents[oi].3 = offs.iter().flat_map(|&x| ord(x, sz)).collect();
        }
        let mut out = Vec::new();
        out.extend_from_slice(if o.le { b"II" } else { b"MM" });
        if o.big {
            out.extend(ord(43, 2));
            out.extend(ord(8, 2));
            out.extend(ord(0, 2));
            out.extend(ord(ifd_at[0] as u64, 8));
        } else {
            out.extend(ord(42, 2));
            out.extend(ord(ifd_at[0] as u64, 4));
        }
        for (k, (_, ents)) in images.iter().enumerate() {
            assert_eq!(out.len(), ifd_at[k]);
            let ifd_len = cnt_sz + ent_sz * ents.len() + off_sz;
            let mut extra = Vec::new();
            out.extend(ord(ents.len() as u64, cnt_sz));
            for x in ents {
                out.extend(ord(x.0 as u64, 2));
                out.extend(ord(x.1 as u64, 2));
                out.extend(ord(x.2, off_sz));
                if x.3.len() > off_sz {
                    out.extend(ord((ifd_at[k] + ifd_len + extra.len()) as u64, off_sz));
                    extra.extend_from_slice(&x.3);
                } else {
                    let mut v = x.3.clone();
                    v.resize(off_sz, 0);
                    out.extend(v);
                }
            }
            out.extend(ord(ifd_at.get(k + 1).map_or(0, |&p| p as u64), off_sz));
            out.extend(extra);
        }
        assert_eq!(out.len(), pos);
        for (blocks, _) in &images {
            for b in blocks {
                out.extend_from_slice(b);
            }
        }
        (out, values)
    }

    /// A value as the sample type stores it, read back as GDAL gives it in an f32 buffer.
    fn sample_value(kind: (u16, u16), v: f64) -> f32 {
        match kind {
            (3, 32) => v as f32,
            (3, 64) => v as f32,
            (2, 16) => (v as i16) as f32,
            (1, 16) => (v as u16) as f32,
            _ => (v as u8) as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testtiff::{make, Opts};
    use super::*;

    fn open(b: Vec<u8>) -> Tiff {
        Tiff::open(Arc::new(b)).unwrap()
    }

    #[test]
    fn every_layout_reads_back() {
        let gt = [-2.0, 0.25, 0.0, 48.0, 0.0, -0.25];
        let (w, h) = (37u32, 29u32);
        let mut n = 0;
        for le in [true, false] {
            for big in [false, true] {
                for tile in [Some(16), None] {
                    for compression in [1, 5, 8, 50000] {
                        for (kind, predictors) in [((3, 32), &[1u16, 2, 3][..]), ((2, 16), &[1, 2][..]), ((1, 16), &[1, 2][..]), ((1, 8), &[1, 2][..])] {
                            for &predictor in predictors {
                                for spp in [1u16, 3] {
                                    let o = Opts { le, big, tile, rows: 6, compression, predictor, kind, spp, keys: &[] };
                                    let (b, vals) = make(w, h, o, gt, Some("-9999"));
                                    let t = open(b);
                                    let got = t.read_window(0, 0, 0, w, h).unwrap();
                                    assert!(got.iter().zip(&vals).all(|(a, b)| a.to_bits() == b.to_bits()), "{o:?}");
                                    // A window across blocks.
                                    let part = t.read_window(0, 5, 3, 20, 17).unwrap();
                                    for y in 0..17 {
                                        for x in 0..20 {
                                            assert_eq!(part[y * 20 + x].to_bits(), vals[(y + 3) * w as usize + x + 5].to_bits(), "{o:?}");
                                        }
                                    }
                                    assert_eq!(t.transform(0).unwrap(), gt);
                                    assert_eq!(t.nodata(), Some(-9999.0));
                                    n += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(n, 2 * 2 * 2 * 4 * 9 * 2);
    }

    #[test]
    fn written_geotiffs_read_back() {
        let (w, h) = (1030u32, 517u32);
        let mut data: Vec<f32> = (0..w * h).map(|i| ((i % 977) as f32 - 300.0) * 1.37).collect();
        data[5] = f32::NAN;
        data[6] = -9999.0;
        let gt = [-2.000138888888889, 0.0002777777777777778, 0.0, 48.00013888888889, 0.0, -0.0002777777777777778];
        let b = write_f32(w, h, &data, 512, gt, 4326, Some(-9999.0)).unwrap();
        assert!(crate::whole::tiff_bytes_whole(&b));
        let t = open(b);
        let got = t.read_window(0, 0, 0, w, h).unwrap();
        assert!(got.iter().zip(&data).all(|(a, b)| a.to_bits() == b.to_bits()));
        assert_eq!(t.transform(0).unwrap(), gt);
        assert_eq!(t.nodata(), Some(-9999.0));
        assert_eq!(t.geo_keys().short(key::GEOGRAPHIC_TYPE), Some(4326));
        assert_eq!(t.geo_keys().ascii(key::GEOG_CITATION), Some("WGS 84"));
        assert_eq!(t.images()[0].block_w, 512);
    }

    #[test]
    fn georeferencing_as_gdal_computes_it() {
        // A PixelIsPoint raster: the tie point is a pixel's centre.
        let (mut b, _) = make(8, 8, Opts::default(), [10.0, 0.5, 0.0, 50.0, 0.0, -0.5], None);
        let t = open(b.clone());
        assert_eq!(t.transform(0).unwrap(), [10.0, 0.5, 0.0, 50.0, 0.0, -0.5]);
        assert_eq!(t.nodata(), None);
        // GeoKeys appended: RasterType = PixelIsPoint. (Rewrite the directory with one more entry.)
        let keys: Vec<u16> = vec![1, 1, 0, 2, key::MODEL_TYPE, 0, 1, 2, key::RASTER_TYPE, 0, 1, 2];
        let at = b.len() as u32;
        for k in &keys {
            b.extend_from_slice(&k.to_le_bytes());
        }
        let n = u16::from_le_bytes([b[8], b[9]]) as usize;
        let ifd_end = 10 + 12 * n;
        let mut ent = Vec::new();
        ent.extend_from_slice(&GEO_KEY_DIRECTORY.to_le_bytes());
        ent.extend_from_slice(&3u16.to_le_bytes());
        ent.extend_from_slice(&(keys.len() as u32).to_le_bytes());
        ent.extend_from_slice(&at.to_le_bytes());
        // Move the directory to the end with the new entry.
        let mut entries: Vec<Vec<u8>> = b[10..ifd_end].chunks(12).map(<[u8]>::to_vec).collect();
        entries.push(ent);
        entries.sort_by_key(|e| u16::from_le_bytes([e[0], e[1]]));
        let new_ifd = b.len() as u32;
        b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for e in entries {
            b.extend(e);
        }
        b.extend_from_slice(&0u32.to_le_bytes());
        b[4..8].copy_from_slice(&new_ifd.to_le_bytes());
        let t = open(b);
        assert_eq!(t.geo_keys().short(key::RASTER_TYPE), Some(2));
        assert_eq!(t.transform(0).unwrap(), [9.75, 0.5, 0.0, 50.25, 0.0, -0.5]);
    }

    #[test]
    fn nodata_text_as_gdal_reads_it() {
        assert_eq!(atof("-32767"), -32767.0);
        assert_eq!(atof(" -9999 \0"), -9999.0);
        assert!(atof("nan").is_nan() && atof("-NaN").is_nan());
        assert_eq!(atof("-inf"), f64::NEG_INFINITY);
        assert_eq!(atof("1e3x"), 1000.0);
        assert_eq!(atof("-3.4028234663852886e+38"), -3.4028234663852886e+38);
        assert_eq!(nodata_text(-9999.0), "-9999");
        assert_eq!(atof(&nodata_text(-3.4028234663852886e38)), -3.4028234663852886e38);
    }

    #[test]
    fn overviews_and_cut_files() {
        // A cut file fails to open or to read, never reads wrong.
        let (b, _) = make(20, 20, Opts { compression: 8, ..Opts::default() }, [0.0, 1.0, 0.0, 0.0, 0.0, -1.0], None);
        for cut in [4usize, 9, 30, b.len() - 10] {
            let r = Tiff::open(Arc::new(b[..cut].to_vec())).and_then(|t| t.read_window(0, 0, 0, 20, 20));
            assert!(r.is_err(), "cut at {cut}");
        }
        let t = open(b);
        assert!(t.level(1).is_err());
        assert_eq!(t.images().len(), 1);
        // Overviews: their own sizes, and the geotransform scaled by the sizes' ratio.
        let gt = [100.0, 2.0, 0.0, 500.0, 0.0, -2.0];
        for big in [false, true] {
            let o = Opts { big, compression: 5, ..Opts::default() };
            let (b, vals) = super::testtiff::make_images(&[(40, 30), (20, 15), (10, 8)], o, gt, Some("-32767"), &|k, x, y, _| (k * 1000) as f64 + (y * 100 + x) as f64);
            let t = open(b);
            assert_eq!(t.images().len(), 3);
            assert_eq!(t.transform(1).unwrap(), [100.0, 4.0, 0.0, 500.0, 0.0, -4.0]);
            assert_eq!(t.transform(2).unwrap(), [100.0, 8.0, 0.0, 500.0, 0.0, -2.0 * 30.0 / 8.0]);
            assert_eq!(t.read_window(2, 0, 0, 10, 8).unwrap(), vals[2]);
            assert_eq!(t.pixel(1, 3, 4).unwrap(), 1403.0);
            assert_eq!(t.nodata(), Some(-32767.0));
        }
    }
}
