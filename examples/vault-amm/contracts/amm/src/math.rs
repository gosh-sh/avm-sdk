//! Dependency-free 256-bit arithmetic and constant-product pricing.
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

/// `floor(sqrt(a * b))`, always representable in `u128`.
pub fn sqrt_product(a: u128, b: u128) -> u128 {
    let target = mul_wide(a, b);
    let (mut low, mut high) = (0u128, u128::MAX);
    while low < high {
        let span = high - low;
        let mid = low + (span >> 1) + (span & 1);
        if mul_wide(mid, mid) <= target {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

/// Compares `a * b` with `c * d` without overflow.
pub fn cmp_products(a: u128, b: u128, c: u128, d: u128) -> Ordering {
    mul_wide(a, b).cmp(&mul_wide(c, d))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PriceError {
    /// A zero amount, zero reserve or an out-of-range fee.
    InvalidInput,
    /// The requested output would drain (or exceed) the output reserve.
    InsufficientLiquidity,
    /// An intermediate or final value does not fit `u128`.
    Overflow,
}

/// The part of `amount_in` that counts towards the invariant after the swap fee.
pub fn amount_after_fee(amount_in: u128, fee_bps: u16) -> Option<u128> {
    mul_div_floor(amount_in, BPS.checked_sub(u128::from(fee_bps))?, BPS)
}

/// Output for an exact input: `floor(r_out * in_f / (r_in + in_f))`.
///
/// `in_f` is `amount_in` reduced by the swap fee, rounded down; the pool always
/// keeps the rounding remainder, so `k` never decreases.
pub fn swap_out(
    reserve_in: u128,
    reserve_out: u128,
    amount_in: u128,
    fee_bps: u16,
) -> Result<u128, PriceError> {
    if amount_in == 0 || reserve_in == 0 || reserve_out == 0 || u128::from(fee_bps) >= BPS {
        return Err(PriceError::InvalidInput);
    }
    let in_f = amount_after_fee(amount_in, fee_bps).ok_or(PriceError::Overflow)?;
    let denominator = reserve_in.checked_add(in_f).ok_or(PriceError::Overflow)?;
    mul_div_floor(reserve_out, in_f, denominator).ok_or(PriceError::Overflow)
}

/// Smallest practical input for an exact output, rounded up twice in the pool's
/// favour: `ceil(ceil(r_in * out / (r_out - out)) * BPS / (BPS - fee))`.
///
/// Feeding the result back into [`swap_out`] returns at least `amount_out`.
pub fn swap_in(
    reserve_in: u128,
    reserve_out: u128,
    amount_out: u128,
    fee_bps: u16,
) -> Result<u128, PriceError> {
    if amount_out == 0 || reserve_in == 0 || reserve_out == 0 || u128::from(fee_bps) >= BPS {
        return Err(PriceError::InvalidInput);
    }
    if amount_out >= reserve_out {
        return Err(PriceError::InsufficientLiquidity);
    }
    let without_fee = mul_div_ceil(reserve_in, amount_out, reserve_out - amount_out)
        .ok_or(PriceError::Overflow)?;
    mul_div_ceil(without_fee, BPS, BPS - u128::from(fee_bps)).ok_or(PriceError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_product_matches_known_values() {
        assert_eq!(mul_wide(0, u128::MAX), U256::ZERO);
        assert_eq!(mul_wide(1, u128::MAX), U256::from_u128(u128::MAX));
        // (2^128 - 1)^2 = 2^256 - 2^129 + 1
        assert_eq!(mul_wide(u128::MAX, u128::MAX), U256 { hi: u128::MAX - 1, lo: 1 });
        assert_eq!(mul_wide(1 << 64, 1 << 64), U256 { hi: 1, lo: 0 });
    }

    #[test]
    fn division_edges() {
        assert_eq!(div_rem(U256::ZERO, 0), None);
        let max = U256 { hi: u128::MAX, lo: u128::MAX };
        assert_eq!(div_rem(max, u128::MAX), Some((U256 { hi: 1, lo: 1 }, 0)));
        assert_eq!(mul_div_floor(u128::MAX, u128::MAX, u128::MAX), Some(u128::MAX));
        assert_eq!(mul_div_floor(u128::MAX, 2, 1), None);
        assert_eq!(mul_div_ceil(7, 3, 2), Some(11));
        assert_eq!(mul_div_floor(7, 3, 2), Some(10));
        assert_eq!(mul_div_ceil(u128::MAX, 1, 1), Some(u128::MAX));
        assert_eq!(mul_div_ceil(u128::MAX, u128::MAX, u128::MAX - 1), None);
    }

    #[test]
    fn square_roots_are_exact_floors() {
        assert_eq!(sqrt_product(0, 5), 0);
        assert_eq!(sqrt_product(1, 1), 1);
        assert_eq!(sqrt_product(2, 2), 2);
        assert_eq!(sqrt_product(2, 3), 2);
        assert_eq!(sqrt_product(u128::MAX, u128::MAX), u128::MAX);
        assert_eq!(sqrt_product(1 << 100, 1 << 100), 1 << 100);
        for value in [1u128, 99, 10_000, 1 << 63, (1 << 64) + 1, u128::MAX / 3] {
            let root = sqrt_product(value, value);
            assert_eq!(root, value);
        }
    }

    #[test]
    fn exact_output_is_never_underpaid() {
        for (r_in, r_out, out, fee) in
            [(1_000u128, 1_000u128, 10u128, 30u16), (10, 1_000_000, 999_999, 1000), (3, 7, 5, 0)]
        {
            let paid = swap_in(r_in, r_out, out, fee).unwrap();
            assert!(swap_out(r_in, r_out, paid, fee).unwrap() >= out);
        }
        assert_eq!(swap_in(5, 5, 5, 30), Err(PriceError::InsufficientLiquidity));
        assert_eq!(swap_in(5, 5, 0, 30), Err(PriceError::InvalidInput));
        assert_eq!(swap_out(5, 5, 0, 30), Err(PriceError::InvalidInput));
    }
}
