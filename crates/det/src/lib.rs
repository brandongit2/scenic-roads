//! Deterministic maths. The transcendental functions come from one implementation (`libm`, a port
//! of musl's) on every target, so the build's outputs are the same bytes on a Mac and in a web
//! worker's WebAssembly (docs/workers.md). The platforms' own libraries differ in the last bit:
//! Apple's and wasi-libc's `sin`, `ln` and `cos` gave different eye heights for 17 of 581,231 road
//! samples in the pilot.
//!
//! The methods are named `d…` because `f64`'s own methods would shadow a trait's of the same name.
//! Exact operations (`sqrt`, `mul_add`, rounding, `powi`, `to_radians`) need nothing: they give the
//! same bits everywhere.

pub trait Det: Sized + Copy {
    fn dsin(self) -> Self;
    fn dcos(self) -> Self;
    fn dtan(self) -> Self;
    fn dasin(self) -> Self;
    fn dacos(self) -> Self;
    fn datan(self) -> Self;
    /// `self.atan2(x)`: the angle of (x, self).
    fn datan2(self, x: Self) -> Self;
    fn dsinh(self) -> Self;
    fn dcosh(self) -> Self;
    fn dtanh(self) -> Self;
    fn dasinh(self) -> Self;
    fn dexp(self) -> Self;
    fn dexp2(self) -> Self;
    fn dexp_m1(self) -> Self;
    fn dln(self) -> Self;
    fn dln_1p(self) -> Self;
    fn dlog2(self) -> Self;
    fn dlog10(self) -> Self;
    fn dpowf(self, y: Self) -> Self;
    fn dhypot(self, y: Self) -> Self;
    fn dcbrt(self) -> Self;
}

macro_rules! det {
    ($t:ty, $sin:ident, $cos:ident, $tan:ident, $asin:ident, $acos:ident, $atan:ident, $atan2:ident, $sinh:ident, $cosh:ident, $tanh:ident, $asinh:ident, $exp:ident, $exp2:ident, $expm1:ident, $log:ident, $log1p:ident, $log2:ident, $log10:ident, $pow:ident, $hypot:ident, $cbrt:ident) => {
        impl Det for $t {
            fn dsin(self) -> $t { libm::$sin(self) }
            fn dcos(self) -> $t { libm::$cos(self) }
            fn dtan(self) -> $t { libm::$tan(self) }
            fn dasin(self) -> $t { libm::$asin(self) }
            fn dacos(self) -> $t { libm::$acos(self) }
            fn datan(self) -> $t { libm::$atan(self) }
            fn datan2(self, x: $t) -> $t { libm::$atan2(self, x) }
            fn dsinh(self) -> $t { libm::$sinh(self) }
            fn dcosh(self) -> $t { libm::$cosh(self) }
            fn dtanh(self) -> $t { libm::$tanh(self) }
            fn dasinh(self) -> $t { libm::$asinh(self) }
            fn dexp(self) -> $t { libm::$exp(self) }
            fn dexp2(self) -> $t { libm::$exp2(self) }
            fn dexp_m1(self) -> $t { libm::$expm1(self) }
            fn dln(self) -> $t { libm::$log(self) }
            fn dln_1p(self) -> $t { libm::$log1p(self) }
            fn dlog2(self) -> $t { libm::$log2(self) }
            fn dlog10(self) -> $t { libm::$log10(self) }
            fn dpowf(self, y: $t) -> $t { libm::$pow(self, y) }
            fn dhypot(self, y: $t) -> $t { libm::$hypot(self, y) }
            fn dcbrt(self) -> $t { libm::$cbrt(self) }
        }
    };
}

det!(f64, sin, cos, tan, asin, acos, atan, atan2, sinh, cosh, tanh, asinh, exp, exp2, expm1, log, log1p, log2, log10, pow, hypot, cbrt);
det!(f32, sinf, cosf, tanf, asinf, acosf, atanf, atan2f, sinhf, coshf, tanhf, asinhf, expf, exp2f, expm1f, logf, log1pf, log2f, log10f, powf, hypotf, cbrtf);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_to_the_platforms() {
        // Within a few ulp of the platform's own (which this replaces for its last bits).
        for x in [0.1f64, 0.7, 1.3, 2.9, 45.0f64.to_radians(), 1e-8, 123.456] {
            for (a, b) in [(x.dsin(), x.sin()), (x.dcos(), x.cos()), (x.dln(), x.ln()), (x.datan(), x.atan()), (x.dsinh(), x.sinh()), (x.dexp(), x.exp())] {
                assert!((a - b).abs() <= 4.0 * f64::EPSILON * b.abs().max(1.0), "{x}: {a} vs {b}");
            }
        }
        assert_eq!(1.0f64.datan2(0.0), std::f64::consts::FRAC_PI_2);
        assert_eq!(3.0f64.dhypot(4.0), 5.0);
    }
}
