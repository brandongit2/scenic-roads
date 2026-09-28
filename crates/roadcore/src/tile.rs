//! Road tile encoding ("RT" v2). Columnar, delta + zigzag varint, gzip'd by the archive.
//!
//!   u8 'R', u8 'T', u8 version (2), u8 log2(extent)
//!   varint nlines, varint nverts
//!   nlines × u8      style byte: class (bits 0-3) | UNPAVED<<4 | BRIDGE<<5 | TUNNEL<<6 | LINK<<7
//!   nlines × varint  zigzag delta of way index (index into ways.bin)
//!   nlines × varint  vertex count
//!   nlines × varint  true (full-resolution) length of the piece, decimetres
//!   nverts × 2 varint  zigzag delta x, y (delta chain runs across lines)
//!   nverts × varint  zigzag delta elevation, decimetres
//!   nverts × u8      |grade| in 0.5 % units
//!   nverts × varint  zigzag delta drape height (terrain surface for 3D), metres
//!   12 × (nverts × varint)  zigzag delta of each scenic channel (roadcore::scenic::ch)
//!
//! Lines are stored minor → major so the client can draw a tile in one ordered pass.

pub const VERSION: u8 = 2;
pub const NCH: usize = crate::scenic::ch::N;

pub mod style {
    pub const UNPAVED: u8 = 1 << 4;
    pub const BRIDGE: u8 = 1 << 5;
    pub const TUNNEL: u8 = 1 << 6;
    pub const LINK: u8 = 1 << 7;
}

#[derive(Default, Clone)]
pub struct TileLine {
    pub style: u8,
    pub way: u32,
    /// Full-resolution length of this piece in decimetres (simplification shortens lines).
    pub true_len_dm: u32,
    pub pts: Vec<[i32; 2]>,
    pub elev: Vec<i16>,
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
