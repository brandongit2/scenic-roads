//! libwebp's combined Shannon entropy (`VP8LCombinedShannonEntropy`: of a histogram X and of
//! X + Y, 256 bins), the lossless encoder's hottest function at the terrain's level (task #888:
//! about a third of a terrain-hi run's CPU, most of it from the cross-colour search, which calls it
//! for every multiplier it tries), replaced by one without a branch a bin. libwebp 1.6 has no NEON
//! version of it (SSE2 and AVX2 only), so on the Macs its plain C ran, which branches on whether
//! each bin is empty: at random in a histogram. Its arithmetic is integer (fixed-point logarithms
//! summed in u64, wrapping as C's unsigned does), so the same sums in another order are the same
//! result to the bit, and the bytes are libwebp's.

/// The bins of the histograms it reads.
const BINS: usize = 256;

type Entropy = unsafe extern "C" fn(*const u32, *const u32) -> u64;

extern "C" {
    /// libwebp's encoder DSP set-up (the C versions chosen, then the SIMD ones built); once it
    /// ran it returns at once (its own guard), so a pointer replaced after it stays.
    fn VP8LEncDspInit();
    static mut VP8LCombinedShannonEntropy: Option<Entropy>;
    /// v·log2(v) in libwebp's fixed point for v < 256 (0 for 0).
    static kSLog2Table: [u64; BINS];
    /// The same for v ≥ 256.
    static VP8LFastSLog2Slow: Option<unsafe extern "C" fn(u32) -> u64>;
}

static INSTALL: std::sync::Once = std::sync::Once::new();
/// libwebp's own function, as its set-up chose it (the tests compare against it).
static OWN: std::sync::OnceLock<Entropy> = std::sync::OnceLock::new();

/// libwebp's DSP set up and its combined entropy replaced, once a process, before any encode.
pub fn install() {
    INSTALL.call_once(|| {
        // SAFETY: the set-up takes no arguments and runs before any encode reads the pointer (an
        // encode here calls this first, and waits on the Once); the pointer is written once,
        // here, and read by libwebp alone.
        unsafe {
            VP8LEncDspInit();
            let own = std::ptr::addr_of!(VP8LCombinedShannonEntropy).read().expect("libwebp's entropy set up");
            let _ = OWN.set(own);
            std::ptr::addr_of_mut!(VP8LCombinedShannonEntropy).write(Some(combined));
        }
    });
}

#[inline(always)]
fn slog2(v: u32) -> u64 {
    if (v as usize) < BINS {
        // SAFETY: a constant table of BINS entries.
        unsafe { kSLog2Table[v as usize] }
    } else {
        // SAFETY: set by VP8LEncDspInit, which `install` ran first; a pure function.
        unsafe { std::ptr::addr_of!(VP8LFastSLog2Slow).read().expect("libwebp's slog2 set up")(v) }
    }
}

/// libwebp's CombinedShannonEntropy_C: Σ over the bins of slog2(x) + slog2(x + y), taken from
/// slog2(Σx) + slog2(Σ(x + y)). Its C skips an empty x (adding slog2(y) alone) and an empty x and
/// y (adding nothing): the same sums, as slog2(0) is 0.
unsafe extern "C" fn combined(x: *const u32, y: *const u32) -> u64 {
    // SAFETY: libwebp passes two histograms of BINS bins.
    let (x, y) = unsafe { (&*(x as *const [u32; BINS]), &*(y as *const [u32; BINS])) };
    let (mut sum_x, mut sum_xy, mut acc) = (0u32, 0u32, 0u64);
    for i in 0..BINS {
        let xy = x[i].wrapping_add(y[i]);
        sum_x = sum_x.wrapping_add(x[i]);
        sum_xy = sum_xy.wrapping_add(xy);
        acc = acc.wrapping_add(slog2(x[i])).wrapping_add(slog2(xy));
    }
    slog2(sum_x).wrapping_add(slog2(sum_xy)).wrapping_sub(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// libwebp's own result on histograms empty, sparse and full, small and large (past the
    /// table, and near u32's top: its wrapping too).
    #[test]
    fn it_is_libwebps_to_the_bit() {
        install();
        let own = *OWN.get().unwrap();
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for case in 0..5000 {
            let (mut x, mut y) = ([0u32; BINS], [0u32; BINS]);
            let fill = [0, 2, 10, 50, 100][case % 5];
            let top = [1u64, 4, 300, 70_000, u32::MAX as u64 / 2][(case / 5) % 5];
            for i in 0..BINS {
                if next() % 100 < fill {
                    x[i] = (next() % top) as u32;
                }
                if next() % 100 < fill {
                    y[i] = (next() % top) as u32;
                }
            }
            // SAFETY: two histograms of BINS bins each.
            let (a, b) = unsafe { (own(x.as_ptr(), y.as_ptr()), combined(x.as_ptr(), y.as_ptr())) };
            assert_eq!(a, b, "case {case}");
        }
    }
}
