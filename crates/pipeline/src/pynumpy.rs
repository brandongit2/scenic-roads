//! numpy's arithmetic as the Python steps ported from it need it, so their outputs stay the bytes
//! the Python made (crate::whsshapes): `default_rng(seed)` (SeedSequence and PCG64) and its
//! `choice(n, size, replace=False)`, the pairwise summation behind `sum` and `mean`, and `arange`
//! and `linspace` of floats. Each is numpy 2.5's algorithm step for step; the tests hold them to
//! numpy's outputs.

/// numpy's `np.random.default_rng(seed)`: PCG64 (XSL-RR) seeded through a SeedSequence.
pub struct Rng {
    state: u128,
    inc: u128,
    /// The upper half of the last 64-bit draw, not yet given out as 32 bits.
    half: Option<u32>,
}

const MULT: u128 = (2549297995355413924u128 << 64) + 4865540595714422341;

/// SeedSequence(seed).generate_state(n, uint64).
fn seed_state(seed: u64, n: usize) -> Vec<u64> {
    const INIT_A: u32 = 0x43b0d7e5;
    const MULT_A: u32 = 0x931e8875;
    const INIT_B: u32 = 0x8b51f9dd;
    const MULT_B: u32 = 0x58f38ded;
    let mut hc = INIT_A;
    let mut hashmix = |v: u32| {
        let mut v = v ^ hc;
        hc = hc.wrapping_mul(MULT_A);
        v = v.wrapping_mul(hc);
        v ^ (v >> 16)
    };
    let mix = |x: u32, y: u32| {
        let r = 0xca01f9ddu32.wrapping_mul(x).wrapping_sub(0x4973f715u32.wrapping_mul(y));
        r ^ (r >> 16)
    };
    let mut ent = vec![seed as u32];
    if seed >> 32 != 0 {
        ent.push((seed >> 32) as u32);
    }
    let mut pool = [0u32; 4];
    for (i, p) in pool.iter_mut().enumerate() {
        *p = hashmix(ent.get(i).copied().unwrap_or(0));
    }
    for s in 0..4 {
        for d in 0..4 {
            if s != d {
                let h = hashmix(pool[s]);
                pool[d] = mix(pool[d], h);
            }
        }
    }
    let mut h = INIT_B;
    let words: Vec<u32> = (0..n * 2)
        .map(|i| {
            let mut v = pool[i % 4] ^ h;
            h = h.wrapping_mul(MULT_B);
            v = v.wrapping_mul(h);
            v ^ (v >> 16)
        })
        .collect();
    (0..n).map(|i| words[2 * i] as u64 | (words[2 * i + 1] as u64) << 32).collect()
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let v = seed_state(seed, 4);
        let init = (v[0] as u128) << 64 | v[1] as u128;
        let seq = (v[2] as u128) << 64 | v[3] as u128;
        let mut r = Rng { state: 0, inc: seq << 1 | 1, half: None };
        r.step();
        r.state = r.state.wrapping_add(init);
        r.step();
        r
    }

    fn step(&mut self) {
        self.state = self.state.wrapping_mul(MULT).wrapping_add(self.inc);
    }

    pub fn next_u64(&mut self) -> u64 {
        self.step();
        let s = self.state;
        (((s >> 64) as u64) ^ s as u64).rotate_right((s >> 122) as u32)
    }

    fn next_u32(&mut self) -> u32 {
        if let Some(h) = self.half.take() {
            return h;
        }
        let n = self.next_u64();
        self.half = Some((n >> 32) as u32);
        n as u32
    }

    /// An integer in 0..=max (numpy's random_bounded_uint64, Lemire's method).
    fn bounded(&mut self, max: u64) -> u64 {
        if max == 0 {
            return 0;
        }
        if max <= u32::MAX as u64 {
            if max == u32::MAX as u64 {
                return self.next_u32() as u64;
            }
            let ex = max + 1;
            let mut m = self.next_u32() as u64 * ex;
            if (m & 0xffff_ffff) < ex {
                let th = (u32::MAX as u64 - max) % ex;
                while (m & 0xffff_ffff) < th {
                    m = self.next_u32() as u64 * ex;
                }
            }
            return m >> 32;
        }
        if max == u64::MAX {
            return self.next_u64();
        }
        let ex = max as u128 + 1;
        let mut m = self.next_u64() as u128 * ex;
        if (m as u64 as u128) < ex {
            let th = (u64::MAX as u128 - max as u128) % ex;
            while (m as u64 as u128) < th {
                m = self.next_u64() as u128 * ex;
            }
        }
        (m >> 64) as u64
    }

    fn shuffle(&mut self, data: &mut [u64], first: usize) {
        for i in (first.max(1)..data.len()).rev() {
            let j = self.bounded(i as u64) as usize;
            data.swap(i, j);
        }
    }

    /// `choice(pop, size, replace=False)`: `size` distinct indexes below `pop`, in numpy's order.
    pub fn choice(&mut self, pop: usize, size: usize) -> Vec<usize> {
        assert!(size <= pop, "a sample larger than its population");
        if pop > 10000 && size > pop / 50 {
            // (A shuffle of the tail.)
            let mut idx: Vec<u64> = (0..pop as u64).collect();
            self.shuffle(&mut idx, pop - size);
            return idx[pop - size..].iter().map(|&i| i as usize).collect();
        }
        // (Floyd's algorithm, with an open-addressed set; then shuffled.)
        let want = (1.2 * size as f64) as u64;
        let mut mask = 1u64;
        while mask < want {
            mask = mask << 1 | 1;
        }
        let mut set = vec![u64::MAX; mask as usize + 1];
        let mut idx = vec![0u64; size];
        for j in (pop - size) as u64..pop as u64 {
            let v = self.bounded(j);
            let mut loc = v & mask;
            while set[loc as usize] != u64::MAX && set[loc as usize] != v {
                loc = (loc + 1) & mask;
            }
            let at = (j as usize + size) - pop;
            if set[loc as usize] == u64::MAX {
                set[loc as usize] = v;
                idx[at] = v;
            } else {
                loc = j & mask;
                while set[loc as usize] != u64::MAX {
                    loc = (loc + 1) & mask;
                }
                set[loc as usize] = j;
                idx[at] = j;
            }
        }
        self.shuffle(&mut idx, 1);
        idx.into_iter().map(|i| i as usize).collect()
    }
}

/// numpy's pairwise summation (`sum` along a contiguous axis), generic over f32 and f64.
macro_rules! pairwise {
    ($name:ident, $t:ty) => {
        pub fn $name(a: &[$t]) -> $t {
            let n = a.len();
            if n < 8 {
                let mut r: $t = -0.0;
                for &x in a {
                    r += x;
                }
                return r;
            }
            if n <= 128 {
                let mut r: [$t; 8] = a[..8].try_into().unwrap();
                let end = n - n % 8;
                let mut i = 8;
                while i < end {
                    for k in 0..8 {
                        r[k] += a[i + k];
                    }
                    i += 8;
                }
                let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
                for &x in &a[end..] {
                    res += x;
                }
                return res;
            }
            let mut n2 = n / 2;
            n2 -= n2 % 8;
            $name(&a[..n2]) + $name(&a[n2..])
        }
    };
}
pairwise!(sum_f32, f32);
pairwise!(sum_f64, f64);

/// `np.mean` of floats.
pub fn mean(a: &[f64]) -> f64 {
    sum_f64(a) / a.len() as f64
}

/// `np.arange(start, stop, step)` of floats: numpy fills `start + i * delta` (fused multiply-add,
/// as its compiled loop does), `delta` being `(start + step) - start`.
pub fn arange(start: f64, stop: f64, step: f64) -> Vec<f64> {
    let n = ((stop - start) / step).ceil();
    if n.is_nan() || n <= 0.0 {
        return vec![];
    }
    let n = n as usize;
    let second = start + step;
    let delta = second - start;
    (0..n)
        .map(|i| match i {
            0 => start,
            1 => second,
            i => (i as f64).mul_add(delta, start),
        })
        .collect()
}

/// `np.linspace(0, stop, n, endpoint=…)`.
pub fn linspace(stop: f64, n: usize, endpoint: bool) -> Vec<f64> {
    let div = if endpoint { n - 1 } else { n } as f64;
    let step = stop / div;
    let mut y: Vec<f64> =
        (0..n).map(|i| if step != 0.0 { i as f64 * step + 0.0 } else { i as f64 / div * stop + 0.0 }).collect();
    if endpoint && n > 1 {
        y[n - 1] = stop;
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    // (The expected values are numpy 2.5.3's.)

    #[test]
    fn pcg64_as_numpy() {
        let mut r = Rng::new(0);
        let raw: Vec<u64> = (0..4).map(|_| r.next_u64()).collect();
        assert_eq!(raw, [11749869230777074271, 4976686463289251617, 755828109848996024, 304881062738325533]);
    }

    #[test]
    fn choice_as_numpy() {
        // Floyd's (a population of up to 10000) and the tail shuffle (larger).
        for (pop, head, sum, wsum) in [
            (5000, [3603, 3033, 2091, 826, 4440, 792, 1978, 4575], 9986712u64, 19902472676u64),
            (66049, [19952, 14869, 5062, 27897, 34880, 23990, 60137, 48761], 132578156, 268404604030),
        ] {
            let c = Rng::new(0).choice(pop, 4000);
            assert_eq!(c[..8], head);
            assert_eq!(c.iter().map(|&i| i as u64).sum::<u64>(), sum);
            assert_eq!(c.iter().enumerate().map(|(k, &i)| (k * i) as u64).sum::<u64>(), wsum);
        }
    }

    #[test]
    fn sums_as_numpy() {
        for (n, s32, s64, m) in [
            (3, 251.0f32, 251.0, 83.66666666666667),
            (8, 676.0, 676.0, 84.5),
            (100, 7007.1435546875, 7007.142857142857, 70.07142857142857),
            (129, 9180.5712890625, 9180.571428571428, 71.16722037652269),
            (1000, 71357.140625, 71357.14285714284, 71.35714285714285),
            (4000, 285428.5625, 285428.57142857136, 71.35714285714285),
        ] {
            let x: Vec<f32> = (0..n).map(|i| ((i * 7919) % 1000) as f32 / 7.0).collect();
            let y: Vec<f64> = (0..n).map(|i| ((i * 7919) % 1000) as f64 / 7.0).collect();
            assert_eq!((sum_f32(&x), sum_f64(&y), mean(&y)), (s32, s64, m), "{n}");
        }
    }

    #[test]
    fn ranges_as_numpy() {
        let (s, h) = (0.4073938373503974, 0.020345525067614962);
        let a = arange(s, s + 79.0 * h - 1e-12, h);
        assert_eq!((a.len(), a[27], a[78]), (79, 0.9567230141760009, 1.9943447926243631));
        assert_eq!(
            linspace(3.3, 7, false),
            [
                0.0,
                0.4714285714285714,
                0.9428571428571428,
                1.4142857142857141,
                1.8857142857142857,
                2.357142857142857,
                2.8285714285714283
            ]
        );
    }
}
