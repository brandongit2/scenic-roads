//! Road and rail tile encoding ("RT" v8). Columnar, delta + zigzag varint, gzip'd by the archive.
//!
//!   u8 'R', u8 'T', u8 version (8), u8 log2(extent)
//!   varint nlines, varint nverts
//!   nlines × u8      style byte: class (bits 0-3) | UNPAVED<<4 | BRIDGE<<5 | TUNNEL<<6 | LINK<<7
//!   nlines × u8      line flags (`lflag`)
//!   nlines × varint  zigzag delta of the OSM way id (v6: index into ways.bin)
//!   nlines × varint  vertex count
//!   nlines × varint  true (full-resolution) length of the piece, decimetres
//!   nlines × varint  length of the whole road the way belongs to, metres (its road value: `codecs::chain`)
//!   nlines × 3 u8    attributes: maxspeed (km/h ÷ 2, 0 = unknown), lanes, surface (`surface`)
//!   nlines × varint  line colour: 0 = none, else 0xRRGGBB + 1 (rail)
//!   nverts × 2 varint  zigzag delta x, y (delta chain runs across lines)
//!   nverts × varint  zigzag delta elevation, decimetres
//!   nverts × u8      |grade| in 0.5 % units
//!   nverts × varint  zigzag delta drape height (terrain surface for 3D), metres
//!   13 × (nverts × varint)  zigzag delta of each scenic channel (roadcore::scenic::ch; v5: 12)
//!
//! Lines are stored minor → major so the client can draw a tile in one ordered pass, and by way id
//! within a draw class.

pub const VERSION: u8 = 8;
pub const NCH: usize = crate::scenic::ch::N;

pub mod style {
    pub const UNPAVED: u8 = 1 << 4;
    pub const BRIDGE: u8 = 1 << 5;
    pub const TUNNEL: u8 = 1 << 6;
    pub const LINK: u8 = 1 << 7;
}

/// Per-line flags (v3), after the styles.
pub mod lflag {
    /// The way has neither a name nor a route number.
    pub const UNNAMED: u8 = 1;
    /// Roads: one-way; toll.
    pub const ONEWAY: u8 = 2;
    pub const TOLL: u8 = 4;
    /// Rail: service groups using the track, bits 1–5 (tram, metro, commuter, intercity, heritage).
    pub const RAIL_SHIFT: u8 = 1;
}

/// Surface codes (per-line attribute 2).
pub mod surface {
    pub const UNKNOWN: u8 = 0;
    pub const ASPHALT: u8 = 1;
    pub const CONCRETE: u8 = 2;
    pub const SETT: u8 = 3;
    pub const COMPACTED: u8 = 4;
    pub const GRAVEL: u8 = 5;
    pub const DIRT: u8 = 6;

    pub fn code(tag: &str) -> u8 {
        match tag {
            "asphalt" | "paved" | "chipseal" => ASPHALT,
            "concrete" | "concrete:plates" | "concrete:lanes" => CONCRETE,
            "paving_stones" | "sett" | "cobblestone" | "unhewn_cobblestone" | "bricks" | "metal" | "wood" => SETT,
            "compacted" | "fine_gravel" => COMPACTED,
            "gravel" | "pebblestone" | "unpaved" | "shells" => GRAVEL,
            "dirt" | "earth" | "ground" | "mud" | "sand" | "grass" | "rock" | "clay" | "woodchips" | "grass_paver"
            | "snow" | "ice" | "salt" => DIRT,
            _ => UNKNOWN,
        }
    }
}

#[derive(Default, Clone)]
pub struct TileLine {
    pub style: u8,
    /// `lflag` bits.
    pub flags: u8,
    pub way: u32,
    /// Full-resolution length of this piece in decimetres (simplification shortens lines).
    pub true_len_dm: u32,
    /// Length of the whole road (chained ways) this line is part of, metres.
    pub road_m: u32,
    /// Maxspeed ÷ 2, lanes, surface code.
    pub attr: [u8; 3],
    /// 0 = none, else 0xRRGGBB + 1.
    pub colour: u32,
    pub pts: Vec<[i32; 2]>,
    /// Decimetres.
    pub elev: Vec<i32>,
    pub grade: Vec<u8>,
    pub drape: Vec<i16>,
    pub sc: Vec<[u8; NCH]>,
}

#[inline]
fn zz(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[inline]
fn put(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub fn encode(lines: &[TileLine], extent_log2: u8) -> Vec<u8> {
    let nverts: usize = lines.iter().map(|l| l.pts.len()).sum();
    let mut out = Vec::with_capacity(16 + lines.len() * 4 + nverts * 6);
    out.extend_from_slice(&[b'R', b'T', VERSION, extent_log2]);
    put(&mut out, lines.len() as u64);
    put(&mut out, nverts as u64);
    out.extend(lines.iter().map(|l| l.style));
    out.extend(lines.iter().map(|l| l.flags));
    let mut pw = 0i64;
    for l in lines {
        put(&mut out, zz(l.way as i64 - pw));
        pw = l.way as i64;
    }
    for l in lines {
        put(&mut out, l.pts.len() as u64);
    }
    for l in lines {
        put(&mut out, l.true_len_dm as u64);
    }
    for l in lines {
        put(&mut out, l.road_m as u64);
    }
    for l in lines {
        out.extend_from_slice(&l.attr);
    }
    for l in lines {
        put(&mut out, l.colour as u64);
    }
    let (mut px, mut py) = (0i64, 0i64);
    for l in lines {
        for p in &l.pts {
            put(&mut out, zz(p[0] as i64 - px));
            put(&mut out, zz(p[1] as i64 - py));
            px = p[0] as i64;
            py = p[1] as i64;
        }
    }
    let mut pe = 0i64;
    for l in lines {
        for &e in &l.elev {
            put(&mut out, zz(e as i64 - pe));
            pe = e as i64;
        }
    }
    for l in lines {
        out.extend_from_slice(&l.grade);
    }
    let mut pd = 0i64;
    for l in lines {
        for &h in &l.drape {
            put(&mut out, zz(h as i64 - pd));
            pd = h as i64;
        }
    }
    for c in 0..NCH {
        let mut pv = 0i64;
        for l in lines {
            for v in &l.sc {
                put(&mut out, zz(v[c] as i64 - pv));
                pv = v[c] as i64;
            }
        }
    }
    out
}

/// Reads a tile `encode` wrote (v8, and v7's attributes of four bytes), with its log2(extent): the
/// server clips the lines of a tile only partly downloaded (crates/server clip.rs).
pub fn decode(b: &[u8]) -> anyhow::Result<(Vec<TileLine>, u8)> {
    use anyhow::{bail, ensure};
    ensure!(b.len() >= 4 && b[0] == b'R' && b[1] == b'T', "not a road tile");
    let (v, ext) = (b[2], b[3]);
    ensure!(v == 7 || v == VERSION, "road tile version {v}");
    let mut pos = 4;
    let get = |b: &[u8], pos: &mut usize| -> anyhow::Result<u64> {
        let (mut r, mut s) = (0u64, 0u32);
        loop {
            let Some(&c) = b.get(*pos) else { bail!("road tile cut short") };
            *pos += 1;
            ensure!(s < 64, "road tile: a varint too long");
            r |= u64::from(c & 0x7f) << s;
            s += 7;
            if c & 0x80 == 0 {
                return Ok(r);
            }
        }
    };
    let unzz = |v: u64| (v >> 1) as i64 ^ -((v & 1) as i64);
    let take = |b: &[u8], pos: &mut usize, n: usize| -> anyhow::Result<Vec<u8>> {
        ensure!(*pos + n <= b.len(), "road tile cut short");
        *pos += n;
        Ok(b[*pos - n..*pos].to_vec())
    };
    let nlines = get(b, &mut pos)? as usize;
    let nverts = get(b, &mut pos)? as usize;
    ensure!(nlines <= b.len() && nverts <= b.len(), "road tile: counts past its size");
    let style = take(b, &mut pos, nlines)?;
    let flags = take(b, &mut pos, nlines)?;
    let mut lines: Vec<TileLine> =
        (0..nlines).map(|i| TileLine { style: style[i], flags: flags[i], ..Default::default() }).collect();
    let mut w = 0i64;
    for l in &mut lines {
        w += unzz(get(b, &mut pos)?);
        l.way = w as u32;
    }
    let mut counts = Vec::with_capacity(nlines);
    for _ in 0..nlines {
        counts.push(get(b, &mut pos)? as usize);
    }
    ensure!(counts.iter().sum::<usize>() == nverts, "road tile: its lines' vertices aren't its count");
    for l in &mut lines {
        l.true_len_dm = get(b, &mut pos)? as u32;
    }
    for l in &mut lines {
        l.road_m = get(b, &mut pos)? as u32;
    }
    let attr_len = if v == 7 { 4 } else { 3 };
    for l in &mut lines {
        let a = take(b, &mut pos, attr_len)?;
        l.attr.copy_from_slice(&a[attr_len - 3..]);
    }
    for l in &mut lines {
        l.colour = get(b, &mut pos)? as u32;
    }
    let (mut px, mut py) = (0i64, 0i64);
    for (l, &n) in lines.iter_mut().zip(&counts) {
        for _ in 0..n {
            px += unzz(get(b, &mut pos)?);
            py += unzz(get(b, &mut pos)?);
            l.pts.push([px as i32, py as i32]);
        }
    }
    let mut pe = 0i64;
    for (l, &n) in lines.iter_mut().zip(&counts) {
        for _ in 0..n {
            pe += unzz(get(b, &mut pos)?);
            l.elev.push(pe as i32);
        }
    }
    for (l, &n) in lines.iter_mut().zip(&counts) {
        l.grade = take(b, &mut pos, n)?;
    }
    let mut pd = 0i64;
    for (l, &n) in lines.iter_mut().zip(&counts) {
        for _ in 0..n {
            pd += unzz(get(b, &mut pos)?);
            l.drape.push(pd as i16);
        }
    }
    for (l, &n) in lines.iter_mut().zip(&counts) {
        l.sc = vec![[0; NCH]; n];
    }
    for c in 0..NCH {
        let mut pv = 0i64;
        for l in &mut lines {
            for s in &mut l.sc {
                pv += unzz(get(b, &mut pos)?);
                s[c] = pv as u8;
            }
        }
    }
    Ok((lines, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile read back is the lines encoded.
    #[test]
    fn decode_reads_what_encode_wrote() {
        let line = |way: u32, pts: Vec<[i32; 2]>| TileLine {
            style: 3,
            flags: lflag::ONEWAY,
            way,
            true_len_dm: 1234,
            road_m: 99,
            attr: [40, 2, surface::GRAVEL],
            colour: 0x123456 + 1,
            elev: (0..pts.len() as i32).map(|i| 100 - 7 * i).collect(),
            grade: (0..pts.len() as u8).collect(),
            drape: (0..pts.len() as i16).map(|i| -3 * i).collect(),
            sc: (0..pts.len()).map(|i| [i as u8; NCH]).collect(),
            pts,
        };
        let lines = vec![line(77, vec![[0, 0], [4096, 10], [-5, 300]]), line(12, vec![[1, 1], [2, 2]])];
        let (back, ext) = decode(&encode(&lines, 12)).unwrap();
        assert_eq!(ext, 12);
        assert_eq!(encode(&back, 12), encode(&lines, 12));
        assert_eq!(back[1].way, 12);
        assert_eq!(back[0].sc[2], [2; NCH]);
    }
}
