//! A quick hasher (FxHash's multiply and rotate) for maps that are only looked in, never walked
//! in their order, so it changes nothing but the time: SipHash was a third of the peaks' coarse
//! floods, and most of the scheduler's look-ups of each target it weighs (pipeline's
//! `coord::sched`, task #995). Not for keys an outsider chooses (no defence against collisions).

use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Default)]
pub struct Fx(u64);

impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..c.len()].copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(b));
        }
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
    fn write_i64(&mut self, v: i64) {
        self.write_u64(v as u64);
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(v as u64);
    }
}

pub type FxBuild = std::hash::BuildHasherDefault<Fx>;
pub type FxSet<K> = HashSet<K, FxBuild>;
pub type FxMap<K, V> = HashMap<K, V, FxBuild>;
