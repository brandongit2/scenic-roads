//! libwebp's combined Shannon entropy (`VP8LCombinedShannonEntropy`: of a histogram X and of
//! X + Y, 256 bins), the lossless encoder's hottest function at the terrain's level (task #888:
//! about a third of a terrain-hi run's CPU, most of it from the cross-colour search, which calls it
//! for every multiplier it tries), replaced by one without the branch on each bin's emptiness
//! (its slog2 still picks table, inline or libwebp's own by the value). libwebp 1.6 has no NEON
//! version of it (SSE2 and AVX2 only), so on the Macs its plain C ran, which branches on whether
//! each bin is empty: at random in a histogram.
//!
//! Why it is libwebp's result on every input, not only the tested ones: per bin its C adds, to a
//! u32 sum of x, a u32 sum of x + y and a u64 sum of slog2s, either x, x + y, slog2(x) and
//! slog2(x + y) (x ≠ 0), or 0, y, 0 and slog2(y) (x = 0, y ≠ 0), or nothing (both 0). This adds
//! x, x + y, slog2(x) and slog2(x + y) always; with x = 0 those are 0, y, slog2(0) = 0 (libwebp's
//! table starts with 0, which a test checks) and slog2(y): the same terms. Every sum is unsigned and
//! wrapping (C's u32 and u64, Rust's wrapping_add), so mod 2^32 and 2^64, where the order of the
//! terms doesn't matter, and the end, slog2(Σx) + slog2(Σ(x + y)) − the sum, wraps the same in
//! both. slog2 is libwebp's: its table below 256; below 65,536 its own integer formula, tested at
//! every one of those values (the shift is 1 to 8, the table index 128 to 255, the product under
//! 2^43: no overflow); from 65,536 libwebp's own function, called.

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
    /// log2(v) in that fixed point, for v < 256.
    static kLog2Table: [u32; BINS];
    /// v·log2(v) for v ≥ 256.
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

/// libwebp's fixed point: log2 in units of 2^-23 (LOG_2_PRECISION_BITS).
const PRECISION_BITS: u64 = 23;
/// 1/ln 2 in that fixed point (LOG_2_RECIPROCAL_FIXED).
const RECIPROCAL_FIXED: u64 = 12_102_203;
/// Past this, libwebp's slog2 takes a floating-point log (APPROX_LOG_WITH_CORRECTION_MAX).
const WITH_CORRECTION_MAX: u32 = 65_536;

/// v·log2(v) as libwebp's VP8LFastSLog2 has it: from its table below 256; its FastSLog2Slow_C's
/// integer approximation below 65,536, here inline (its own is a call through a pointer, and
/// histograms summed over a tile often pass 256); past that its own (a floating-point log, left
/// to libwebp's C so the libm is the same).
#[inline(always)]
fn slog2(v: u32) -> u64 {
    if (v as usize) < BINS {
        // SAFETY: a constant table of BINS entries.
        unsafe { kSLog2Table[v as usize] }
    } else if v < WITH_CORRECTION_MAX {
        let log_cnt = (31 - v.leading_zeros()) - 7;
        let y = 1u32 << log_cnt;
        // SAFETY: v >> log_cnt is under 256 (v's top bit lands on bit 7).
        let log2 = unsafe { kLog2Table[(v >> log_cnt) as usize] } as u64;
        v as u64 * (log2 + ((log_cnt as u64) << PRECISION_BITS)) + RECIPROCAL_FIXED * (v & (y - 1)) as u64
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

    /// libwebp's own slog2 for every value its inline copy answers, and some past them.
    #[test]
    fn slog2_is_libwebps() {
        install();
        // (The proof above leans on slog2(0) being 0.)
        // SAFETY: a constant table.
        assert_eq!(unsafe { kSLog2Table[0] }, 0);
        // SAFETY: set up by `install`; a pure function of v ≥ 256.
        let own = unsafe { std::ptr::addr_of!(VP8LFastSLog2Slow).read().unwrap() };
        for v in (BINS as u32..WITH_CORRECTION_MAX + 4096).chain([1 << 20, 123_456_789, u32::MAX]) {
            // SAFETY: as above.
            assert_eq!(slog2(v), unsafe { own(v) }, "{v}");
        }
    }

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
