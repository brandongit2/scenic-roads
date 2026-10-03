//! Processed elevations per vertex, in decimetres, as stored: `final.i16` and base packs' `elev`
//! (signed: −3,276.8 to 3,276.7 m; what was built before 2026-10-03), or `final.u16` and `elevu`
//! (unsigned from −500 m, `OFF`: up to 6,053.5 m, as roads in the Andes and the Himalaya need).
//! Readers take either; writers write the unsigned form.

use crate::Array;
use anyhow::Result;
use std::ops::Range;
use std::path::Path;

/// What's added to an elevation (dm) to store it unsigned.
pub const OFF: i32 = 5000;
/// The elevations (dm) the unsigned form holds.
pub const MIN_DM: i32 = -OFF;
pub const MAX_DM: i32 = u16::MAX as i32 - OFF;

/// An elevation (dm) as stored unsigned, clamped to the range it holds.
pub fn to_u16(dm: i32) -> u16 {
    (dm + OFF).clamp(0, u16::MAX as i32) as u16
}

/// A run of stored elevations.
#[derive(Clone, Copy, Debug)]
pub enum Elevs<'a> {
    I16(&'a [i16]),
    U16(&'a [u16]),
}

impl<'a> Elevs<'a> {
    pub fn len(&self) -> usize {
        match self {
            Elevs::I16(s) => s.len(),
            Elevs::U16(s) => s.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Vertex `i`'s elevation, decimetres.
    pub fn dm(&self, i: usize) -> i32 {
        match self {
            Elevs::I16(s) => s[i] as i32,
            Elevs::U16(s) => s[i] as i32 - OFF,
        }
    }
    pub fn slice(&self, r: Range<usize>) -> Elevs<'a> {
        match self {
            Elevs::I16(s) => Elevs::I16(&s[r]),
            Elevs::U16(s) => Elevs::U16(&s[r]),
        }
    }
    /// Vertex `i`'s elevation, metres.
    pub fn m(&self, i: usize) -> f32 {
        self.dm(i) as f32 / 10.0
    }
    /// Every elevation, decimetres.
    pub fn to_dm(&self) -> Vec<i32> {
        (0..self.len()).map(|i| self.dm(i)).collect()
    }
}

/// A build folder's processed elevations, mapped: `final.u16`, else `final.i16`.
pub enum Stored {
    I16(Array<i16>),
    U16(Array<u16>),
}

impl Stored {
    pub fn open(dir: &Path) -> Result<Stored> {
        let u = dir.join("final.u16");
        Ok(if u.exists() { Stored::U16(Array::open(&u)?) } else { Stored::I16(Array::open(&dir.join("final.i16"))?) })
    }
    pub fn get(&self) -> Elevs<'_> {
        match self {
            Stored::I16(a) => Elevs::I16(a.get()),
            Stored::U16(a) => Elevs::U16(a.get()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_forms_read_alike() {
        let a: Vec<i16> = vec![-5000, 0, 12345, 32767];
        let b: Vec<u16> = a.iter().map(|&d| to_u16(d as i32)).collect();
        assert_eq!(Elevs::I16(&a).to_dm(), Elevs::U16(&b).to_dm());
        // Past i16: the Andes.
        assert_eq!(Elevs::U16(&[to_u16(58_000)]).dm(0), 58_000);
        assert_eq!(to_u16(-6000), 0);
        assert_eq!(to_u16(70_000), u16::MAX);
        assert_eq!(Elevs::U16(&[to_u16(MIN_DM), to_u16(MAX_DM)]).to_dm(), vec![MIN_DM, MAX_DM]);
    }
}
