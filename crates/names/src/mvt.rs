//! Mapbox Vector Tiles (spec 2.1), as far as rewriting properties needs: decoding and encoding,
//! attaching `main` and `sub` to named features as tiles are served, and merging tiles.
//!
//! Nothing is lost on the way through: fields the spec doesn't define (extensions, or a known
//! field with an unexpected wire type) are kept as raw bytes and written back, values that aren't
//! exactly one known field are kept whole ([`Value::Other`]), and [`attach`] rewrites only the
//! layers it changes, copying every other byte of the tile as it was. Tiles here are raw protobuf;
//! [`gunzip_if_gzip`] and [`gzip`] convert.

use det::Det;
use crate::area::area_at;
use crate::display::{Kind, Names};
use crate::pbf::{packed_u32, put_bytes, put_key, put_packed, put_uint, unzigzag, zigzag, Field, Reader, Wire};
use anyhow::{anyhow, bail, Context, Result};
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};

/// The property [`attach`] gives a named feature's label.
pub const MAIN: &str = "main";
/// The property [`attach`] gives the line under the label, when there is one.
pub const SUB: &str = "sub";

/// The largest tile [`gunzip_if_gzip`] unpacks.
const MAX_TILE: u64 = 256 << 20;

/// A decoded tile.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tile {
    pub layers: Vec<Layer>,
    /// Fields other than layers, as read.
    pub unknown: Vec<u8>,
}

/// A layer: its features' properties are pairs of indices into `keys` and `values`.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    pub version: u32,
    pub extent: u32,
    pub keys: Vec<String>,
    pub values: Vec<Value>,
    pub features: Vec<Feature>,
    /// Fields the spec doesn't define, as read.
    pub unknown: Vec<u8>,
}

/// A feature.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Feature {
    pub id: Option<u64>,
    /// Key and value indices, alternating.
    pub tags: Vec<u32>,
    /// 1 point, 2 line string, 3 polygon (0 or absent: unknown).
    pub geom_type: Option<u32>,
    /// The packed command integers, as read.
    pub geometry: Vec<u32>,
    /// Fields the spec doesn't define, as read.
    pub unknown: Vec<u8>,
}

/// A property value. Equality and hashing compare floats by their bits.
#[derive(Clone, Debug)]
pub enum Value {
    String(String),
    Float(f32),
    Double(f64),
    Int(i64),
    Uint(u64),
    Sint(i64),
    Bool(bool),
    /// A value message that isn't exactly one well-formed known field, kept whole.
    Other(Vec<u8>),
}

impl Value {
    fn decode(bytes: &[u8]) -> Value {
        let mut r = Reader::new(bytes);
        let mut found = None;
        let mut fields = 0;
        loop {
            let f = match r.next_field() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(_) => return Value::Other(bytes.to_vec()),
            };
            fields += 1;
            found = match (f.num, f.wire) {
                (1, Wire::Bytes(s)) => std::str::from_utf8(s).ok().map(|s| Value::String(s.to_owned())),
                (2, Wire::Fixed32(x)) => Some(Value::Float(f32::from_bits(x))),
                (3, Wire::Fixed64(x)) => Some(Value::Double(f64::from_bits(x))),
                (4, Wire::Varint(x)) => Some(Value::Int(x as i64)),
                (5, Wire::Varint(x)) => Some(Value::Uint(x)),
                (6, Wire::Varint(x)) => Some(Value::Sint(unzigzag(x))),
                (7, Wire::Varint(x)) if x <= 1 => Some(Value::Bool(x == 1)),
                _ => None,
            };
            if found.is_none() {
                break;
            }
        }
        match found {
            Some(v) if fields == 1 => v,
            _ => Value::Other(bytes.to_vec()),
        }
    }

    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Value::String(s) => put_bytes(out, 1, s.as_bytes()),
            Value::Float(x) => {
                put_key(out, 2, 5);
                out.extend_from_slice(&x.to_bits().to_le_bytes());
            }
            Value::Double(x) => {
                put_key(out, 3, 1);
                out.extend_from_slice(&x.to_bits().to_le_bytes());
            }
            Value::Int(x) => put_uint(out, 4, *x as u64),
            Value::Uint(x) => put_uint(out, 5, *x),
            Value::Sint(x) => put_uint(out, 6, zigzag(*x)),
            Value::Bool(x) => put_uint(out, 7, u64::from(*x)),
            Value::Other(raw) => out.extend_from_slice(raw),
        }
    }

    /// The string, if this is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        use Value::*;
        match (self, other) {
            (String(a), String(b)) => a == b,
            (Float(a), Float(b)) => a.to_bits() == b.to_bits(),
            (Double(a), Double(b)) => a.to_bits() == b.to_bits(),
            (Int(a), Int(b)) | (Sint(a), Sint(b)) => a == b,
            (Uint(a), Uint(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (Other(a), Other(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, h: &mut H) {
        std::mem::discriminant(self).hash(h);
        match self {
            Value::String(s) => s.hash(h),
            Value::Float(x) => x.to_bits().hash(h),
            Value::Double(x) => x.to_bits().hash(h),
            Value::Int(x) | Value::Sint(x) => x.hash(h),
            Value::Uint(x) => x.hash(h),
            Value::Bool(x) => x.hash(h),
            Value::Other(raw) => raw.hash(h),
        }
    }
}

impl Feature {
    fn decode(bytes: &[u8]) -> Result<Feature> {
        let mut f = Feature::default();
        let mut r = Reader::new(bytes);
        while let Some(field) = r.next_field()? {
            match (field.num, field.wire) {
                (1, Wire::Varint(v)) => f.id = Some(v),
                (2, Wire::Bytes(b)) => packed_u32(b, &mut f.tags)?,
                (2, Wire::Varint(v)) => f.tags.push(u32::try_from(v).context("tag over 32 bits")?),
                (3, Wire::Varint(v)) => f.geom_type = Some(u32::try_from(v).context("geometry type over 32 bits")?),
                (4, Wire::Bytes(b)) => packed_u32(b, &mut f.geometry)?,
                (4, Wire::Varint(v)) => f.geometry.push(u32::try_from(v).context("geometry over 32 bits")?),
                _ => f.unknown.extend_from_slice(field.raw),
            }
        }
        Ok(f)
    }

    fn encode(&self, out: &mut Vec<u8>) {
        if let Some(id) = self.id {
            put_uint(out, 1, id);
        }
        put_packed(out, 2, &self.tags);
        if let Some(t) = self.geom_type {
            put_uint(out, 3, u64::from(t));
        }
        put_packed(out, 4, &self.geometry);
        out.extend_from_slice(&self.unknown);
    }

    /// The first point of the geometry, in tile coordinates (the start of its first MoveTo).
    pub fn first_point(&self) -> Option<(i32, i32)> {
        match self.geometry.as_slice() {
            [cmd, x, y, ..] if cmd & 7 == 1 && cmd >> 3 >= 1 => Some((unzigzag(u64::from(*x)) as i32, unzigzag(u64::from(*y)) as i32)),
            _ => None,
        }
    }
}

impl Layer {
    /// Decodes a layer message.
    pub fn decode(bytes: &[u8]) -> Result<Layer> {
        let mut l = Layer { name: String::new(), version: 1, extent: 4096, keys: Vec::new(), values: Vec::new(), features: Vec::new(), unknown: Vec::new() };
        let mut named = false;
        let mut r = Reader::new(bytes);
        while let Some(field) = r.next_field()? {
            match (field.num, field.wire) {
                (1, Wire::Bytes(b)) => {
                    l.name = String::from_utf8(b.to_vec()).context("layer name isn't UTF-8")?;
                    named = true;
                }
                (2, Wire::Bytes(b)) => l.features.push(Feature::decode(b)?),
                (3, Wire::Bytes(b)) => l.keys.push(String::from_utf8(b.to_vec()).context("key isn't UTF-8")?),
                (4, Wire::Bytes(b)) => l.values.push(Value::decode(b)),
                (5, Wire::Varint(v)) => l.extent = u32::try_from(v).context("extent over 32 bits")?,
                (15, Wire::Varint(v)) => l.version = u32::try_from(v).context("version over 32 bits")?,
                _ => l.unknown.extend_from_slice(field.raw),
            }
        }
        if !named {
            bail!("a layer without a name");
        }
        Ok(l)
    }

    /// Encodes the layer message (fields in number order, then those not understood).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, 1, self.name.as_bytes());
        let mut buf = Vec::new();
        for f in &self.features {
            buf.clear();
            f.encode(&mut buf);
            put_bytes(&mut out, 2, &buf);
        }
        for k in &self.keys {
            put_bytes(&mut out, 3, k.as_bytes());
        }
        for v in &self.values {
            buf.clear();
            v.encode(&mut buf);
            put_bytes(&mut out, 4, &buf);
        }
        put_uint(&mut out, 5, u64::from(self.extent));
        put_uint(&mut out, 15, u64::from(self.version));
        out.extend_from_slice(&self.unknown);
        out
    }
}

impl Tile {
    /// Decodes a tile (raw protobuf, not gzip'd).
    pub fn decode(bytes: &[u8]) -> Result<Tile> {
        let mut t = Tile::default();
        let mut r = Reader::new(bytes);
        while let Some(field) = r.next_field()? {
            match (field.num, field.wire) {
                (3, Wire::Bytes(b)) => t.layers.push(Layer::decode(b)?),
                _ => t.unknown.extend_from_slice(field.raw),
            }
        }
        Ok(t)
    }

    /// Encodes the tile: its layers, then the fields not understood.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for l in &self.layers {
            put_bytes(&mut out, 3, &l.encode());
        }
        out.extend_from_slice(&self.unknown);
        out
    }
}

/// A tile coordinate (`px`, `py` in a layer of `extent`, in tile `z`/`x`/`y`) as longitude and
/// latitude (Web Mercator). Points in the buffer beyond the tile's edge work too.
pub fn tile_to_lonlat(z: u32, x: u32, y: u32, extent: u32, px: f64, py: f64) -> (f64, f64) {
    let n = f64::from(z).dexp2();
    let e = f64::from(extent);
    let wx = (f64::from(x) + px / e) / n;
    let wy = (f64::from(y) + py / e) / n;
    (wx * 360.0 - 180.0, (PI * (1.0 - 2.0 * wy)).dsinh().datan().to_degrees())
}

/// Tile `z`/`x`/`y`'s bounds as `[west, south, east, north]` in degrees, grown by `buffer` tiles on
/// every side (and kept within the Web Mercator world): the box to give [`crate::areas_in`] for the
/// areas whose versions a tile's ETag must hold. Features are read where their first point is,
/// which can lie outside the tile: up to a whole tile out for the OpenMapTiles basemap's place,
/// water and park label points (their label buffer), so use 1.0 there; our label tiles keep each
/// label in its own tile (0.0).
pub fn tile_bounds(z: u32, x: u32, y: u32, buffer: f64) -> [f64; 4] {
    let n = f64::from(z).dexp2();
    let lon = |t: f64| (t / n).clamp(0.0, 1.0) * 360.0 - 180.0;
    let lat = |t: f64| (PI * (1.0 - 2.0 * (t / n).clamp(0.0, 1.0))).dsinh().datan().to_degrees();
    let (x, y) = (f64::from(x), f64::from(y));
    [lon(x - buffer), lat(y + 1.0 + buffer), lon(x + 1.0 + buffer), lat(y - buffer)]
}

/// Which layers [`attach`] names, and where a feature's name and own English are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerRule<'a> {
    /// The layer's name, or `"*"` for every layer. The first rule matching a layer applies.
    pub layer: &'a str,
    /// The properties holding the name, the first present (a non-empty string) wins.
    pub name_keys: &'a [&'a str],
    /// The properties holding the thing's own English, likewise.
    pub en_keys: &'a [&'a str],
}

/// The OpenMapTiles basemap: `name`, with OSM's English in `name:en` or `name_en`.
pub const OPENMAPTILES: LayerRule<'static> = LayerRule { layer: "*", name_keys: &["name"], en_keys: &["name:en", "name_en"] };

/// Our label tiles (layer `l`): `n`, with English in `en`.
pub const LABELS: LayerRule<'static> = LayerRule { layer: "l", name_keys: &["n"], en_keys: &["en"] };

/// Gives every named feature of the layers `rules` match its display name, as string properties
/// [`MAIN`] and (when there is one) [`SUB`], replacing any it had: the name read as a place
/// ([`Kind::Place`]: map labels name places, water, parks and the like, never roads) in the area at
/// the feature's first point, with its own English (see [`Names::display`]).
///
/// `tile` is raw protobuf (not gzip'd) for tile `z`/`x`/`y`. Returns the new tile, or `None` when
/// nothing changed (no named features, or all already as they should be): serve the original
/// bytes. Layers it doesn't change, and malformed layers, are copied byte for byte. Fails only when
/// the tile itself can't be read; serve it as it is then.
pub fn attach(tile: &[u8], z: u32, x: u32, y: u32, names: &Names, rules: &[LayerRule]) -> Result<Option<Vec<u8>>> {
    if tile.starts_with(&[0x1f, 0x8b]) {
        bail!("the tile is gzip'd (gunzip_if_gzip it first)");
    }
    if z > 30 || x >> z != 0 || y >> z != 0 {
        bail!("{z}/{x}/{y} is not a tile");
    }
    let fields = fields(tile)?;
    let mut rewritten: Vec<Option<Vec<u8>>> = vec![None; fields.len()];
    for (i, f) in fields.iter().enumerate() {
        let (3, Wire::Bytes(body)) = (f.num, f.wire) else { continue };
        let Some(name) = layer_name(body) else { continue };
        let Some(rule) = rules.iter().find(|r| r.layer == "*" || r.layer.as_bytes() == name) else { continue };
        let Ok(mut layer) = Layer::decode(body) else { continue };
        if attach_layer(&mut layer, rule, (z, x, y), names) {
            rewritten[i] = Some(layer.encode());
        }
    }
    if rewritten.iter().all(Option::is_none) {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(tile.len() + tile.len() / 4);
    for (f, new) in fields.iter().zip(rewritten) {
        match new {
            Some(body) => put_bytes(&mut out, 3, &body),
            None => out.extend_from_slice(f.raw),
        }
    }
    Ok(Some(out))
}

/// A tile's top-level fields.
fn fields(tile: &[u8]) -> Result<Vec<Field<'_>>> {
    let mut r = Reader::new(tile);
    let mut out = Vec::new();
    while let Some(f) = r.next_field().context("not a vector tile")? {
        out.push(f);
    }
    Ok(out)
}

/// A layer message's name (its last `name` field, as a decoder would take it).
fn layer_name(body: &[u8]) -> Option<&[u8]> {
    let mut r = Reader::new(body);
    let mut name = None;
    while let Ok(Some(f)) = r.next_field() {
        if let (1, Wire::Bytes(b)) = (f.num, f.wire) {
            name = Some(b);
        }
    }
    name
}

/// [`attach`] for one decoded layer; whether anything changed.
fn attach_layer(layer: &mut Layer, rule: &LayerRule, (z, x, y): (u32, u32, u32), names: &Names) -> bool {
    let rank = |list: &[&str]| -> Vec<Option<usize>> { layer.keys.iter().map(|k| list.iter().position(|c| c == k)).collect() };
    let name_rank = rank(rule.name_keys);
    if name_rank.iter().all(Option::is_none) {
        return false;
    }
    let en_rank = rank(rule.en_keys);
    let is_main: Vec<bool> = layer.keys.iter().map(|k| k == MAIN).collect();
    let is_sub: Vec<bool> = layer.keys.iter().map(|k| k == SUB).collect();

    // What each named feature should carry, where it doesn't yet.
    let mut plans: Vec<(usize, String, Option<String>)> = Vec::new();
    for (i, f) in layer.features.iter().enumerate() {
        if f.tags.len() % 2 != 0 {
            continue;
        }
        let mut name: Option<(usize, &str)> = None;
        let mut en: Option<(usize, &str)> = None;
        let (mut mains, mut subs) = (Vec::new(), Vec::new());
        let mut valid = true;
        for pair in f.tags.chunks_exact(2) {
            let (k, v) = (pair[0] as usize, pair[1] as usize);
            let Some(value) = layer.values.get(v).filter(|_| k < layer.keys.len()) else {
                valid = false;
                break;
            };
            let s = value.as_str().filter(|s| !s.is_empty());
            if let (Some(r), Some(s)) = (name_rank[k], s) {
                if name.is_none_or(|(best, _)| r < best) {
                    name = Some((r, s));
                }
            }
            if let (Some(r), Some(s)) = (en_rank[k], s) {
                if en.is_none_or(|(best, _)| r < best) {
                    en = Some((r, s));
                }
            }
            if is_main[k] {
                mains.push(value);
            }
            if is_sub[k] {
                subs.push(value);
            }
        }
        let (true, Some((_, name))) = (valid, name) else { continue };
        let at = f.first_point().filter(|_| layer.extent > 0).map(|(px, py)| tile_to_lonlat(z, x, y, layer.extent, f64::from(px), f64::from(py)));
        let d = names.display_in(Kind::Place, at.and_then(|(lon, lat)| area_at(lon, lat)), name, en.map(|(_, s)| s));
        let has = |vals: &[&Value], want: Option<&str>| match (vals, want) {
            ([], None) => true,
            ([v], Some(w)) => v.as_str() == Some(w),
            _ => false,
        };
        if !(has(&mains, Some(d.main)) && has(&subs, d.sub)) {
            plans.push((i, d.main.to_owned(), d.sub.map(str::to_owned)));
        }
    }
    if plans.is_empty() {
        return false;
    }

    let old_keys = layer.keys.len();
    let mut key = |k: &str| -> u32 {
        let i = layer.keys.iter().position(|c| c == k).unwrap_or_else(|| {
            layer.keys.push(k.to_owned());
            layer.keys.len() - 1
        });
        i as u32
    };
    let main_key = key(MAIN);
    let sub_key = plans.iter().any(|p| p.2.is_some()).then(|| key(SUB));
    let mut strings: HashMap<String, u32> = HashMap::new();
    for (i, v) in layer.values.iter().enumerate().rev() {
        if let Value::String(s) = v {
            strings.insert(s.clone(), i as u32);
        }
    }
    let values = &mut layer.values;
    let mut value = |s: String| -> u32 {
        *strings.entry(s).or_insert_with_key(|s| {
            values.push(Value::String(s.clone()));
            (values.len() - 1) as u32
        })
    };
    for (i, main, sub) in plans {
        let (mv, sv) = (value(main), sub.map(&mut value));
        let f = &mut layer.features[i];
        let mut tags: Vec<u32> = Vec::with_capacity(f.tags.len() + 4);
        for pair in f.tags.chunks_exact(2) {
            let k = pair[0] as usize;
            if !(k < old_keys && (is_main[k] || is_sub[k])) {
                tags.extend_from_slice(pair);
            }
        }
        tags.extend([main_key, mv]);
        if let (Some(sk), Some(sv)) = (sub_key, sv) {
            tags.extend([sk, sv]);
        }
        f.tags = tags;
    }
    true
}

/// Merges tiles of the same z/x/y: layers of the same name become one (features in tile order,
/// keys and values re-indexed), others are copied as they are, in order of first appearance.
/// Same-named layers must agree on extent; the merged layer takes the lowest version.
pub fn merge(tiles: &[&[u8]]) -> Result<Vec<u8>> {
    let all: Vec<Vec<Field>> = tiles.iter().map(|t| fields(t)).collect::<Result<_>>()?;
    let mut by_name: HashMap<&[u8], Vec<&[u8]>> = HashMap::new();
    for f in all.iter().flatten() {
        if let (3, Wire::Bytes(body)) = (f.num, f.wire) {
            let name = layer_name(body).ok_or_else(|| anyhow!("a layer without a name"))?;
            by_name.entry(name).or_default().push(body);
        }
    }
    let mut out = Vec::new();
    let mut done: HashSet<&[u8]> = HashSet::new();
    for f in all.iter().flatten() {
        let (3, Wire::Bytes(body)) = (f.num, f.wire) else {
            out.extend_from_slice(f.raw);
            continue;
        };
        let name = layer_name(body).unwrap_or_default();
        let bodies = by_name.get(name).map(Vec::as_slice).unwrap_or_default();
        if bodies.len() <= 1 {
            out.extend_from_slice(f.raw);
        } else if done.insert(name) {
            put_bytes(&mut out, 3, &merge_layers(bodies)?.encode());
        }
    }
    Ok(out)
}

fn merge_layers(bodies: &[&[u8]]) -> Result<Layer> {
    let layers: Vec<Layer> = bodies.iter().map(|b| Layer::decode(b)).collect::<Result<_>>()?;
    let first = &layers[0];
    let mut m = Layer { name: first.name.clone(), version: first.version, extent: first.extent, keys: Vec::new(), values: Vec::new(), features: Vec::new(), unknown: Vec::new() };
    let mut keys: HashMap<String, u32> = HashMap::new();
    let mut values: HashMap<Value, u32> = HashMap::new();
    for l in layers {
        if l.extent != m.extent {
            bail!("layer {}: extents differ ({} and {})", m.name, m.extent, l.extent);
        }
        m.version = m.version.min(l.version);
        let kmap: Vec<u32> = l
            .keys
            .into_iter()
            .map(|k| {
                *keys.entry(k).or_insert_with_key(|k| {
                    m.keys.push(k.clone());
                    (m.keys.len() - 1) as u32
                })
            })
            .collect();
        let vmap: Vec<u32> = l
            .values
            .into_iter()
            .map(|v| {
                *values.entry(v).or_insert_with_key(|v| {
                    m.values.push(v.clone());
                    (m.values.len() - 1) as u32
                })
            })
            .collect();
        for mut f in l.features {
            if f.tags.len() % 2 != 0 {
                bail!("layer {}: a feature with an odd number of tags", m.name);
            }
            for pair in f.tags.chunks_exact_mut(2) {
                let (Some(k), Some(v)) = (kmap.get(pair[0] as usize), vmap.get(pair[1] as usize)) else {
                    bail!("layer {}: a feature's tags point past its keys or values", m.name);
                };
                (pair[0], pair[1]) = (*k, *v);
            }
            m.features.push(f);
        }
        m.unknown.extend_from_slice(&l.unknown);
    }
    Ok(m)
}

/// The tile unzipped when it is gzip'd (by its magic number), else as it is.
pub fn gunzip_if_gzip(data: &[u8]) -> Result<Cow<'_, [u8]>> {
    if !data.starts_with(&[0x1f, 0x8b]) {
        return Ok(Cow::Borrowed(data));
    }
    let mut out = Vec::with_capacity(data.len() * 4);
    MultiGzDecoder::new(data).take(MAX_TILE + 1).read_to_end(&mut out).context("gunzip")?;
    if out.len() as u64 > MAX_TILE {
        bail!("tile over {} MiB unzipped", MAX_TILE >> 20);
    }
    Ok(Cow::Owned(out))
}

/// Gzips a tile (default level).
pub fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut e = GzEncoder::new(Vec::with_capacity(data.len() / 3 + 64), Compression::default());
    e.write_all(data)?;
    Ok(e.finish()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.to_owned())
    }

    /// A point feature at tile coordinates (px, py).
    fn point(id: u64, tags: &[u32], px: i32, py: i32) -> Feature {
        Feature { id: Some(id), tags: tags.to_vec(), geom_type: Some(1), geometry: vec![9, zigzag(px.into()) as u32, zigzag(py.into()) as u32], unknown: Vec::new() }
    }

    fn layer(name: &str, keys: &[&str], values: Vec<Value>, features: Vec<Feature>) -> Layer {
        Layer { name: name.to_owned(), version: 2, extent: 4096, keys: keys.iter().map(|k| (*k).to_owned()).collect(), values, features, unknown: Vec::new() }
    }

    /// Feature `i`'s properties by name.
    fn props(l: &Layer, i: usize) -> Vec<(String, Value)> {
        l.features[i].tags.chunks_exact(2).map(|p| (l.keys[p[0] as usize].clone(), l.values[p[1] as usize].clone())).collect()
    }

    fn prop(l: &Layer, i: usize, k: &str) -> Option<Value> {
        props(l, i).into_iter().find(|(kk, _)| kk == k).map(|(_, v)| v)
    }

    #[test]
    fn round_trip_every_value_kind() {
        let values = vec![s("a"), Value::Float(1.5), Value::Float(f32::NAN), Value::Double(-0.0), Value::Int(-7), Value::Uint(u64::MAX), Value::Sint(-300), Value::Bool(true), Value::Bool(false)];
        let mut l = layer("water", &["a", "b"], values, vec![point(1, &[0, 0, 1, 2], 100, -5)]);
        l.features.push(Feature { id: None, tags: vec![], geom_type: None, geometry: vec![], unknown: vec![0x2a, 0x01, 0x07] }); // field 5, bytes
        l.unknown = vec![0x80, 0x01, 0x01]; // field 16, varint 1 (an extension)
        let t = Tile { layers: vec![l, layer("empty", &[], vec![], vec![])], unknown: vec![0x08, 0x05] };
        let bytes = t.encode();
        let back = Tile::decode(&bytes).expect("decode");
        assert_eq!(back, t);
        assert_eq!(back.encode(), bytes);
    }

    #[test]
    fn odd_values_are_kept_whole() {
        let mut raw = Vec::new();
        put_bytes(&mut raw, 1, b"x");
        put_uint(&mut raw, 4, 3); // two fields
        let cases = [raw, vec![0x38, 0x02] /* bool 2 */, vec![0x0a, 0x01, 0xff] /* not UTF-8 */, vec![0x40, 0x01] /* field 8 */, vec![0x0a, 0x05] /* truncated */, vec![]];
        for raw in cases {
            let v = Value::decode(&raw);
            assert_eq!(v, Value::Other(raw.clone()));
            let mut out = Vec::new();
            v.encode(&mut out);
            assert_eq!(out, raw);
        }
    }

    #[test]
    fn decodes_unpacked_and_rejects_broken() {
        // Tags and geometry as repeated varints rather than packed.
        let mut f = Vec::new();
        put_uint(&mut f, 2, 0);
        put_uint(&mut f, 2, 0);
        put_uint(&mut f, 4, 9);
        put_uint(&mut f, 4, 2);
        put_uint(&mut f, 4, 4);
        let f = Feature::decode(&f).expect("decode");
        assert_eq!((f.tags, f.geometry), (vec![0, 0], vec![9, 2, 4]));
        assert!(Tile::decode(&[0x1a, 0x05, 0x0a]).is_err());
        let mut nameless = Vec::new();
        put_uint(&mut nameless, 15, 2);
        let mut t = Vec::new();
        put_bytes(&mut t, 3, &nameless);
        assert!(Tile::decode(&t).is_err());
    }

    #[test]
    fn coordinates() {
        let (lon, lat) = tile_to_lonlat(0, 0, 0, 4096, 2048.0, 2048.0);
        assert!(lon.abs() < 1e-9 && lat.abs() < 1e-9);
        let (lon, lat) = tile_to_lonlat(1, 1, 0, 4096, 0.0, 4096.0);
        assert!(lon.abs() < 1e-9 && lat.abs() < 1e-9);
        let (lon, lat) = tile_to_lonlat(0, 0, 0, 4096, 0.0, 0.0);
        assert!((lon + 180.0).abs() < 1e-9 && (lat - 85.0511287798).abs() < 1e-6);
        assert_eq!(point(0, &[], -3, 7).first_point(), Some((-3, 7)));
        let line = Feature { geometry: vec![9, 4, 4, 18, 2, 2, 4, 4], ..Feature::default() };
        assert_eq!(line.first_point(), Some((2, 2)));
        assert_eq!(Feature { geometry: vec![2, 4, 4], ..Feature::default() }.first_point(), None);
        assert_eq!(Feature::default().first_point(), None);
        // Bounds: the whole world at z0; a z1 tile grown by half a tile; clamped at the edges.
        let [w, s, e, n] = tile_bounds(0, 0, 0, 0.0);
        assert!((w + 180.0).abs() < 1e-9 && (e - 180.0).abs() < 1e-9 && (s + 85.0511287798).abs() < 1e-6 && (n - 85.0511287798).abs() < 1e-6);
        let [w, s, e, n] = tile_bounds(1, 1, 0, 0.5);
        assert!((w + 90.0).abs() < 1e-9 && (e - 180.0).abs() < 1e-9 && (n - 85.0511287798).abs() < 1e-6);
        assert!((s - tile_to_lonlat(1, 1, 0, 4096, 0.0, 4096.0 * 1.5).1).abs() < 1e-9 && s < 0.0);
    }

    /// Tile coordinates of (lon, lat) in z/x/y at extent 4096.
    fn at(z: u32, x: u32, y: u32, lon: f64, lat: f64) -> (i32, i32) {
        let n = f64::from(z).dexp2();
        let wx = (lon + 180.0) / 360.0 * n - f64::from(x);
        let wy = (1.0 - lat.to_radians().dtan().dasinh() / PI) / 2.0 * n - f64::from(y);
        ((wx * 4096.0).round() as i32, (wy * 4096.0).round() as i32)
    }

    fn names() -> (crate::testdir::Dir, Names) {
        let d = crate::testdir::Dir::new();
        d.write("jp/places-jp.jsonl", "{\"n\": \"松島\", \"main\": \"松島\", \"sub\": \"Matsu-shima\"}\n{\"n\": \"東京\", \"en\": null}\n");
        d.write("fr/places-fr.jsonl", "{\"n\": \"Église\", \"main\": \"Church\", \"sub\": null}\n");
        let n = Names::load(&d.0).expect("load");
        (d, n)
    }

    #[test]
    fn attaches_main_and_sub() {
        let (_d, names) = names();
        // z5 tile 28/12 holds northern Honshu; 16/11 holds Paris.
        let (mx, my) = at(5, 28, 12, 141.06, 38.37);
        let (tx, ty) = at(5, 28, 12, 139.69, 35.69);
        let jp = layer(
            "place",
            &["name", "name:en", "class"],
            vec![s("松島"), s("Matsushima"), s("town"), s("東京"), s("Tokyo"), s("仙台"), s("Sendai"), Value::Int(3)],
            vec![
                point(1, &[0, 0, 1, 1, 2, 2], mx, my),
                point(2, &[0, 3, 1, 4], tx, ty),
                point(3, &[0, 5, 1, 6], mx, my),
                point(4, &[2, 2], mx, my),
                point(5, &[0, 7], mx, my),
            ],
        );
        let other = layer("roads", &["name"], vec![s("松島")], vec![point(9, &[0, 0], mx, my)]);
        let tile = Tile { layers: vec![jp, other.clone()], unknown: vec![] };
        let bytes = tile.encode();
        let rules = [LayerRule { layer: "place", ..OPENMAPTILES }];
        let out = attach(&bytes, 5, 28, 12, &names, &rules).expect("attach").expect("changed");
        let t = Tile::decode(&out).expect("decode");
        let l = &t.layers[0];
        // A translation line: its sub, over OSM's English.
        assert_eq!(prop(l, 0, MAIN), Some(s("松島")));
        assert_eq!(prop(l, 0, SUB), Some(s("Matsu-shima")));
        // A line with no sub: none, whatever OSM has.
        assert_eq!(prop(l, 1, MAIN), Some(s("東京")));
        assert_eq!(prop(l, 1, SUB), None);
        // No line: the thing's own English.
        assert_eq!(prop(l, 2, MAIN), Some(s("仙台")));
        assert_eq!(prop(l, 2, SUB), Some(s("Sendai")));
        // Unnamed, or named by a number: untouched.
        assert_eq!(props(l, 3), props(&tile.layers[0], 3));
        assert_eq!(props(l, 4), props(&tile.layers[0], 4));
        // Everything else as it was; layers no rule names copied as they were.
        for i in 0..3 {
            let before = props(&tile.layers[0], i);
            assert_eq!(props(l, i).into_iter().filter(|(k, _)| k != MAIN && k != SUB).collect::<Vec<_>>(), before);
        }
        assert_eq!(t.layers[1], other);
        assert!(out.ends_with(&bytes[bytes.len() - other.encode().len()..]));
        // The name's own value is reused for main.
        assert_eq!(l.values.iter().filter(|v| v.as_str() == Some("松島")).count(), 1);

        // Done again: nothing changes.
        assert_eq!(attach(&out, 5, 28, 12, &names, &rules).expect("attach"), None);
        // Stale main and sub are replaced (and a sub removed).
        let mut stale = t.clone();
        let sk = stale.layers[0].keys.iter().position(|k| k == SUB).expect("sub key") as u32;
        stale.layers[0].values.push(s("Wrong"));
        let wrong = (stale.layers[0].values.len() - 1) as u32;
        stale.layers[0].features[1].tags.extend([sk, wrong]);
        let fixed = attach(&stale.encode(), 5, 28, 12, &names, &rules).expect("attach").expect("changed");
        let fixed = Tile::decode(&fixed).expect("decode");
        assert_eq!(prop(&fixed.layers[0], 1, SUB), None);
        assert_eq!(props(&fixed.layers[0], 1).iter().filter(|(k, _)| k == MAIN).count(), 1);
    }

    #[test]
    fn attach_reads_the_area_where_the_feature_is() {
        let (_d, names) = names();
        // Église in Paris is a church; the same name in Tokyo's tile has no line there.
        let (px, py) = at(5, 16, 11, 2.35, 48.86);
        let paris = Tile { layers: vec![layer("l", &["n", "en"], vec![s("Église"), s("Saint Church")], vec![point(1, &[0, 0, 1, 1], px, py)])], unknown: vec![] };
        let out = attach(&paris.encode(), 5, 16, 11, &names, &[LABELS]).expect("attach").expect("changed");
        let t = Tile::decode(&out).expect("decode");
        assert_eq!((prop(&t.layers[0], 0, MAIN), prop(&t.layers[0], 0, SUB)), (Some(s("Church")), None));
        let (px, py) = at(5, 28, 12, 139.69, 35.69);
        let tokyo = Tile { layers: vec![layer("l", &["n", "en"], vec![s("Église"), s("Saint Church")], vec![point(1, &[0, 0, 1, 1], px, py)])], unknown: vec![] };
        let out = attach(&tokyo.encode(), 5, 28, 12, &names, &[LABELS]).expect("attach").expect("changed");
        let t = Tile::decode(&out).expect("decode");
        assert_eq!((prop(&t.layers[0], 0, MAIN), prop(&t.layers[0], 0, SUB)), (Some(s("Église")), Some(s("Saint Church"))));
    }

    #[test]
    fn attach_leaves_what_it_cannot_read() {
        let (_d, names) = names();
        // No named features: None.
        let plain = Tile { layers: vec![layer("water", &["class"], vec![s("lake")], vec![point(1, &[0, 0], 1, 1)])], unknown: vec![] };
        assert_eq!(attach(&plain.encode(), 0, 0, 0, &names, &[OPENMAPTILES]).expect("attach"), None);
        // A broken layer is copied as it is, the good one still named.
        let good = layer("place", &["name"], vec![s("Paris")], vec![point(1, &[0, 0], 1, 1)]);
        let mut broken = Vec::new();
        put_bytes(&mut broken, 1, b"bad");
        put_bytes(&mut broken, 3, &[0xff]); // a key that isn't UTF-8
        let mut bytes = Vec::new();
        put_bytes(&mut bytes, 3, &broken);
        put_bytes(&mut bytes, 3, &good.encode());
        bytes.extend_from_slice(&[0x80, 0x01, 0x05]); // field 16
        let out = attach(&bytes, 0, 0, 0, &names, &[OPENMAPTILES]).expect("attach").expect("changed");
        assert!(out.starts_with(&bytes[..broken.len() + 2]));
        assert!(out.ends_with(&[0x80, 0x01, 0x05]));
        // Bad input.
        assert!(attach(&gzip(&bytes).expect("gzip"), 0, 0, 0, &names, &[OPENMAPTILES]).is_err());
        assert!(attach(&bytes, 1, 2, 0, &names, &[OPENMAPTILES]).is_err());
        assert!(attach(&[0x1a, 0x10, 0x00], 0, 0, 0, &names, &[OPENMAPTILES]).is_err());
        // Features with broken tags are skipped.
        let odd = layer("place", &["name"], vec![s("Paris")], vec![point(1, &[0, 0, 1], 1, 1), point(2, &[0, 5], 1, 1)]);
        assert_eq!(attach(&Tile { layers: vec![odd], unknown: vec![] }.encode(), 0, 0, 0, &names, &[OPENMAPTILES]).expect("attach"), None);
    }

    #[test]
    fn merges() {
        let a = layer("water", &["class", "name"], vec![s("lake"), s("Lac"), Value::Int(1)], vec![point(1, &[0, 0, 1, 1], 1, 1)]);
        let mut b = layer("water", &["name", "class"], vec![s("river"), s("Lac")], vec![point(2, &[1, 0, 0, 1], 2, 2)]);
        b.version = 1;
        b.unknown = vec![0x80, 0x01, 0x01];
        let c = layer("place", &["name"], vec![s("Paris")], vec![point(3, &[0, 0], 3, 3)]);
        let t1 = Tile { layers: vec![a.clone(), c.clone()], unknown: vec![0x08, 0x01] };
        let t2 = Tile { layers: vec![b.clone()], unknown: vec![] };
        let merged = merge(&[&t1.encode(), &t2.encode()]).expect("merge");
        let m = Tile::decode(&merged).expect("decode");
        assert_eq!(m.layers.len(), 2);
        let w = &m.layers[0];
        assert_eq!((w.name.as_str(), w.version, w.keys.len(), w.values.len(), w.features.len()), ("water", 1, 2, 4, 2));
        assert_eq!(props(w, 0), props(&a, 0));
        assert_eq!(props(w, 1), props(&b, 0));
        assert_eq!(w.unknown, b.unknown);
        assert_eq!(m.layers[1], c);
        assert_eq!(m.unknown, vec![0x08, 0x01]);
        // One tile: as it was.
        let one = t1.encode();
        assert_eq!(merge(&[&one]).expect("merge"), one);
        assert!(merge(&[]).expect("merge").is_empty());
        // Extents must agree.
        let mut d = c.clone();
        d.extent = 512;
        let t3 = Tile { layers: vec![d], unknown: vec![] };
        assert!(merge(&[&t1.encode(), &t3.encode()]).is_err());
    }

    #[test]
    fn gzip_helpers() {
        let raw = b"\x1a\x03abc".to_vec();
        assert!(matches!(gunzip_if_gzip(&raw).expect("plain"), Cow::Borrowed(_)));
        let gz = gzip(&raw).expect("gzip");
        assert_eq!(gunzip_if_gzip(&gz).expect("gunzip").as_ref(), raw.as_slice());
        assert!(gunzip_if_gzip(&[0x1f, 0x8b, 0x08, 0x00]).is_err());
    }
}
