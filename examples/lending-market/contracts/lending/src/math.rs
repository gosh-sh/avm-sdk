//! Dependency-free 256-bit arithmetic and the interest-rate model.
//!
//! Every function is pure, deterministic and checked: no operation panics or
//! wraps silently. Results that do not fit `u128` return `None`/`Err`.

use core::cmp::Ordering;

/// Basis-point denominator.
pub const BPS: u128 = 10_000;

/// An unsigned 256-bit integer, most significant half first.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct U256 {
    pub hi: u128,
    pub lo: u128,
}

impl U256 {
    pub const ZERO: Self = Self { hi: 0, lo: 0 };

    pub const fn from_u128(value: u128) -> Self {
        Self { hi: 0, lo: value }
    }

    fn bit(self, index: u32) -> bool {
        if index >= 128 {
            (self.hi >> (index - 128)) & 1 == 1
        } else {
            (self.lo >> index) & 1 == 1
        }
    }

    fn set_bit(&mut self, index: u32) {
        if index >= 128 {
            self.hi |= 1 << (index - 128);
        } else {
            self.lo |= 1 << index;
        }
    }
}

impl Ord for U256 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.hi.cmp(&other.hi).then(self.lo.cmp(&other.lo))
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Full 128 x 128 -> 256 bit product.
pub fn mul_wide(a: u128, b: u128) -> U256 {
    const MASK: u128 = u64::MAX as u128;
    let (a0, a1) = (a & MASK, a >> 64);
    let (b0, b1) = (b & MASK, b >> 64);
    let ll = a0 * b0;
    let lh = a0 * b1;
    let hl = a1 * b0;
    let hh = a1 * b1;
    let (mid, carry_a) = lh.overflowing_add(hl);
    let (mid, carry_b) = mid.overflowing_add(ll >> 64);
    let lo = (ll & MASK) | (mid << 64);
    // hh + (mid >> 64) + (carries << 64) < 2^128 for every input.
    let hi = hh + (mid >> 64) + ((u128::from(carry_a) + u128::from(carry_b)) << 64);
    U256 { hi, lo }
}

/// Divides a 256-bit numerator by a nonzero 128-bit divisor.
///
/// Returns the 256-bit quotient and the remainder, or `None` for a zero divisor.
pub fn div_rem(numerator: U256, divisor: u128) -> Option<(U256, u128)> {
    if divisor == 0 {
        return None;
    }
    let mut quotient = U256::ZERO;
    let mut remainder: u128 = 0;
    // Shift-subtract division. It avoids the 128-bit division builtins, which
    // would add about 1.2 KB to the guest; a high half of zero skips 128 steps.
    let bits = if numerator.hi == 0 { 128 } else { 256 };
    for index in (0..bits).rev() {
        let carry = remainder >> 127;
        remainder = (remainder << 1) | u128::from(numerator.bit(index));
        // With the carry set the true remainder is 2^128 + remainder, which
        // always exceeds the divisor; the wrapping subtraction is then exact.
        if carry == 1 || remainder >= divisor {
            remainder = remainder.wrapping_sub(divisor);
            quotient.set_bit(index);
        }
    }
    Some((quotient, remainder))
}

/// `floor(a * b / c)`; `None` if `c == 0` or the result exceeds `u128`.
pub fn mul_div_floor(a: u128, b: u128, c: u128) -> Option<u128> {
    let (quotient, _) = div_rem(mul_wide(a, b), c)?;
    (quotient.hi == 0).then_some(quotient.lo)
}

/// `ceil(a * b / c)`; `None` if `c == 0` or the result exceeds `u128`.
pub fn mul_div_ceil(a: u128, b: u128, c: u128) -> Option<u128> {
    let (quotient, remainder) = div_rem(mul_wide(a, b), c)?;
    if quotient.hi != 0 {
        return None;
    }
    if remainder == 0 {
        Some(quotient.lo)
    } else {
        quotient.lo.checked_add(1)
    }
}

/// Fixed-point unit of the borrow index and of prices (1e18).
pub const ONE: u128 = 1_000_000_000_000_000_000;
/// Seconds in a (365-day) year.
pub const YEAR: u128 = 31_536_000;

/// Annual borrow rate in basis points for a utilisation in basis points: a
/// linear ramp from `base` to `base + slope1` up to the kink, then a steeper
/// one adding up to `slope2` at full utilisation. All inputs are small, so
/// native 64-bit division suffices.
pub fn borrow_rate_bps(util: u64, kink: u64, base: u64, slope1: u64, slope2: u64) -> u64 {
    let low = util.min(kink);
    let mut rate = base + slope1 * low / kink;
    if util > kink {
        rate += slope2 * (util - kink) / (10_000 - kink);
    }
    rate
}

/// Grows a borrow index over `dt` seconds at an annual `rate_bps`
/// (simple growth per accrual; compounding happens at every call).
pub fn grow_index(index: u128, rate_bps: u64, dt: u32) -> Option<u128> {
    let growth = mul_div_floor(index, u128::from(rate_bps) * u128::from(dt), YEAR * 10_000)?;
    index.checked_add(growth)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u128 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            u128::from(self.0)
        }
        fn wide(&mut self) -> u128 {
            match self.next() % 4 {
                0 => self.next(),
                1 => (self.next() << 64) | self.next(),
                2 => u128::MAX - self.next() % 7,
                _ => self.next() % 1_000_003,
            }
        }
    }

    #[test]
    fn mul_wide_matches_known_products() {
        assert_eq!(mul_wide(0, u128::MAX), U256::ZERO);
        assert_eq!(mul_wide(u128::MAX, u128::MAX), U256 { hi: u128::MAX - 1, lo: 1 });
        assert_eq!(mul_wide(1 << 64, 1 << 64), U256 { hi: 1, lo: 0 });
        assert_eq!(mul_wide(7, 6), U256::from_u128(42));
    }

    #[test]
    fn division_inverts_multiplication() {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        for _ in 0..4_000 {
            let (a, b, c) = (rng.wide(), rng.wide(), rng.wide().max(1));
            let product = mul_wide(a, b);
            let (quotient, remainder) = div_rem(product, c).unwrap();
            assert!(remainder < c);
            // quotient * c + remainder == a * b, checked limb by limb.
            let low = mul_wide(quotient.lo, c);
            let high = mul_wide(quotient.hi, c);
            assert_eq!(high.hi, 0, "a*b cannot exceed 256 bits");
            let (sum_lo, carry) = low.lo.overflowing_add(remainder);
            let (mid, carry_mid) = low.hi.overflowing_add(high.lo);
            let (mid, carry_carry) = mid.overflowing_add(u128::from(carry));
            assert!(!carry_mid && !carry_carry);
            assert_eq!((mid, sum_lo), (product.hi, product.lo));
        }
        assert_eq!(div_rem(U256::from_u128(5), 0), None);
    }

    #[test]
    fn floor_and_ceil_agree_with_native_arithmetic_when_it_fits() {
        let mut rng = Rng(0xdead_beef_cafe_f00d);
        for _ in 0..4_000 {
            let (a, b, c) =
                (rng.next() % (1 << 60), rng.next() % (1 << 60), rng.next() % 1_000_000);
            let c = c.max(1);
            let exact = a * b;
            assert_eq!(mul_div_floor(a, b, c), Some(exact / c));
            assert_eq!(mul_div_ceil(a, b, c), Some(exact.div_ceil(c)));
        }
        assert_eq!(mul_div_floor(1, 1, 0), None);
        assert_eq!(mul_div_floor(u128::MAX, u128::MAX, 1), None); // result too large
        assert_eq!(mul_div_floor(u128::MAX, u128::MAX, u128::MAX), Some(u128::MAX));
        assert_eq!(mul_div_ceil(u128::MAX, 1, 2), Some(u128::MAX / 2 + 1));
        assert_eq!(mul_div_ceil(u128::MAX, 1, 1), Some(u128::MAX));
    }

    #[test]
    fn rates_are_monotonic_and_bounded() {
        let mut last = 0;
        for util in 0..=10_000u64 {
            let rate = borrow_rate_bps(util, 8_000, 200, 400, 6_000);
            assert!(rate >= last && rate <= 200 + 400 + 6_000);
            last = rate;
        }
        // The extreme configuration still fits comfortably.
        assert_eq!(borrow_rate_bps(10_000, 9_999, 5_000, 5_000, 50_000), 60_000);
        assert_eq!(borrow_rate_bps(10_000, 1, 0, 0, 50_000), 50_000);
    }

    #[test]
    fn the_index_grows_linearly_in_time_and_never_shrinks() {
        assert_eq!(grow_index(ONE, 0, 1_000_000), Some(ONE));
        assert_eq!(grow_index(ONE, 1_000, 31_536_000), Some(ONE + ONE / 10));
        let mut index = ONE;
        for _ in 0..1_000 {
            let next = grow_index(index, 60_000, 86_400).unwrap();
            assert!(next >= index);
            index = next;
            if index > ONE * 1_000_000 {
                break;
            }
        }
        assert!(grow_index(u128::MAX, 60_000, u32::MAX).is_none());
    }
}
