//! Processed elevations per vertex, in decimetres, as stored: `final.u16` and base packs' `elevu`
//! (unsigned from −500 m, `OFF`: up to 6,053.5 m, as roads in the Andes and the Himalaya need).

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
pub struct Elevs<'a>(pub &'a [u16]);

impl<'a> Elevs<'a> {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Vertex `i`'s elevation, decimetres.
    pub fn dm(&self, i: usize) -> i32 {
        self.0[i] as i32 - OFF
    }
    pub fn slice(&self, r: Range<usize>) -> Elevs<'a> {
        Elevs(&self.0[r])
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

/// A build folder's processed elevations, mapped: `final.u16`.
pub struct Stored(Array<u16>);

impl Stored {
    pub fn open(dir: &Path) -> Result<Stored> {
        Ok(Stored(Array::open(&dir.join("final.u16"))?))
    }
    pub fn get(&self) -> Elevs<'_> {
        Elevs(self.0.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevations_read_back_as_stored() {
        let a: Vec<i32> = vec![-5000, 0, 12345, 32767];
        let b: Vec<u16> = a.iter().map(|&d| to_u16(d)).collect();
        assert_eq!(Elevs(&b).to_dm(), a);
        // Past i16: the Andes.
        assert_eq!(Elevs(&[to_u16(58_000)]).dm(0), 58_000);
        assert_eq!(to_u16(-6000), 0);
        assert_eq!(to_u16(70_000), u16::MAX);
        assert_eq!(Elevs(&[to_u16(MIN_DM), to_u16(MAX_DM)]).to_dm(), vec![MIN_DM, MAX_DM]);
    }
}
