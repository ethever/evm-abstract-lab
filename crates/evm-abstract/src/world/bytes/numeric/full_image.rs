//! A fixed-width certificate for the union of all 32 numeric byte projections.

use crate::domain::{Domain, NumericValue, Profile};
use alloy_primitives::U256;

/// Only inspect policy and inline enum tags before the proof's payment.
/// The existing complete-numeric-Top shortcut must precede this candidate.
pub(super) fn candidate(value: &NumericValue, domain: Domain) -> bool {
    domain.spec().profile() == Profile::Product
        && !domain.spec().relations().enabled
        && value.finite_constants().is_top()
        && value.congruence().is_top()
}

/// After candidate admission/payment, both interval hulls must contain the
/// entire known-bit cube. A free byte lane r gives each b>0 the witness
/// ones | (b << 8r). Zero uses ones unless
/// nonzero excludes it; if ones=0, set one free bit in r and read another lane.
/// These are numeric-support witnesses, independent of path reachability.
///
/// Full AV parity also follows from the ordinary transfer's normal form. A
/// free lane yields exactly BYTE(0, numeric Top). If its finite component is
/// Top, it absorbs the other lanes. If it retains all 256 candidates, its
/// successful export (256 candidates + four scalar slots + origins) has room
/// for every constrained lane (at most 128 candidates + one extra NonZero).
/// One round therefore retains each lane's complete subset of 0..255. Ordered
/// joins reproduce the free lane, including finite representation, at any
/// positive round/fact capacity. Every lane has identical transferred sources;
/// the free lane preserves an inherited symbolic-limit marker, while ordinary
/// BYTE clears identity, expression and code-address role for all lanes.
pub(super) fn proves(value: &NumericValue) -> bool {
    let zero = value.known_bits().zero();
    let one = value.known_bits().one();
    if one & zero != U256::ZERO {
        return false;
    }
    let (unsigned_lo, unsigned_hi) = value.interval().unsigned_bounds();
    if unsigned_lo > one || !zero > unsigned_hi {
        return false;
    }
    let sign = U256::from(1) << 255usize;
    let biased_one = (one & !sign) | (zero & sign);
    let biased_zero = (zero & !sign) | (one & sign);
    let (signed_lo, signed_hi) = value.interval().signed_bounds();
    if signed_lo > biased_one || !biased_zero > signed_hi {
        return false;
    }
    // A fixed stack array and at most 32 byte comparisons; no candidate
    // enumeration, membership query, digit DP or metadata copy is needed.
    (!(one | zero)).to_le_bytes::<32>().contains(&u8::MAX)
}
