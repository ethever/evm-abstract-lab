//! Mask 与普通区间的精确存在性查询。四个 tight 状态避免指数枚举未知位。
use super::{NumericValue, known_bits::KnownBits};
use alloy_primitives::U256;
use std::collections::BTreeSet;

type Memo = [[[bool; 2]; 2]; 257];
fn fill_memo(lo: U256, hi: U256, bits: KnownBits, memo: &mut Memo) {
    memo[256] = [[true; 2]; 2];
    // Each suffix depends only on the next depth. Bottom-up evaluation keeps
    // the call stack constant, with at most 256 * 4 * 2 digit checks.
    for depth in (0..256).rev() {
        let index = 255 - depth;
        for lower_tight in [false, true] {
            for upper_tight in [false, true] {
                memo[depth][usize::from(lower_tight)][usize::from(upper_tight)] =
                    [false, true].into_iter().any(|bit| {
                        if (bits.zero().bit(index) && bit)
                            || (bits.one().bit(index) && !bit)
                            || (lower_tight && !bit && lo.bit(index))
                            || (upper_tight && bit && !hi.bit(index))
                        {
                            return false;
                        }
                        let next_lower = lower_tight && bit == lo.bit(index);
                        let next_upper = upper_tight && bit == hi.bit(index);
                        memo[depth + 1][usize::from(next_lower)][usize::from(next_upper)]
                    });
            }
        }
    }
}
fn extreme(lo: U256, hi: U256, bits: KnownBits, maximum: bool, memo: &Memo) -> Option<U256> {
    if !memo[0][1][1] {
        return None;
    }
    let (mut lower_tight, mut upper_tight, mut result) = (true, true, U256::ZERO);
    for depth in 0..256 {
        let index = 255 - depth;
        for bit in [maximum, !maximum] {
            if (bits.zero().bit(index) && bit)
                || (bits.one().bit(index) && !bit)
                || (lower_tight && !bit && lo.bit(index))
                || (upper_tight && bit && !hi.bit(index))
            {
                continue;
            }
            let next_lower = lower_tight && bit == lo.bit(index);
            let next_upper = upper_tight && bit == hi.bit(index);
            if memo[depth + 1][usize::from(next_lower)][usize::from(next_upper)] {
                if bit {
                    result |= U256::from(1) << index;
                }
                lower_tight = next_lower;
                upper_tight = next_upper;
                break;
            }
        }
    }
    Some(result)
}
pub(super) fn mask_bounds(lo: U256, hi: U256, bits: KnownBits) -> Option<(U256, U256)> {
    mask_bounds_with_memo(lo, hi, bits, || [[[false; 2]; 2]; 257])
}

fn mask_bounds_with_memo(
    lo: U256,
    hi: U256,
    bits: KnownBits,
    new_memo: impl FnOnce() -> Memo,
) -> Option<(U256, U256)> {
    if lo > hi {
        return None;
    }
    let lower_matches = bits.contains(lo);
    if lo == hi {
        return lower_matches.then_some((lo, hi));
    }
    let upper_matches = bits.contains(hi);
    if lower_matches && upper_matches {
        return Some((lo, hi));
    }
    // 可行性只依赖 depth/tight、区间和位约束，与极值方向无关。
    let mut memo = new_memo();
    fill_memo(lo, hi, bits, &mut memo);
    let lower = if lower_matches {
        lo
    } else {
        extreme(lo, hi, bits, false, &memo)?
    };
    let upper = if upper_matches {
        hi
    } else {
        extreme(lo, hi, bits, true, &memo)?
    };
    Some((lower, upper))
}

fn bit_pattern_count(value: &NumericValue, capacity: usize) -> Option<usize> {
    let count = (!(value.bits.zero() | value.bits.one())).count_ones();
    if count >= usize::BITS as usize {
        return None;
    }
    let patterns = 1usize << count;
    (patterns <= capacity).then_some(patterns)
}

struct IntervalCandidates {
    pieces: Vec<(U256, U256, U256)>,
}

fn interval_candidates(value: &NumericValue, capacity: usize) -> Option<IntervalCandidates> {
    let (mut count, mut pieces) = (0usize, Vec::new());
    for (lo, hi) in value.interval.segments() {
        let Some((first, last)) = value.congruence.first_last(lo, hi) else {
            continue;
        };
        let step = value
            .congruence
            .modulus_residue()
            .map_or(U256::from(1), |(m, _)| m);
        let size = (last - first) / step;
        if size >= U256::from(capacity) {
            return None;
        }
        count = count.checked_add(usize::try_from(size).ok()?.checked_add(1)?)?;
        if count > capacity {
            return None;
        }
        pieces.push((first, last, step));
    }
    Some(IntervalCandidates { pieces })
}

/// 只返回完整候选覆盖。容量不足返回 None，绝不取前 N 项冒充全体。
pub(super) fn candidates(value: &NumericValue, capacity: usize) -> Option<BTreeSet<U256>> {
    if let Some(set) = value.constants() {
        if set.len() > capacity {
            return None;
        }
        return Some(set.iter().copied().filter(|v| value.contains(*v)).collect());
    }
    if let Some(single) = value.singleton() {
        return Some(BTreeSet::from([single]));
    }
    let unknown = !(value.bits.zero() | value.bits.one());
    if let Some(patterns) = bit_pattern_count(value, capacity) {
        let positions = (0..256).filter(|bit| unknown.bit(*bit)).collect::<Vec<_>>();
        let mut out = BTreeSet::new();
        for pattern in 0..patterns {
            let mut candidate = value.bits.one();
            for (i, bit) in positions.iter().enumerate() {
                if pattern & (1usize << i) != 0 {
                    candidate |= U256::from(1) << *bit;
                }
            }
            if value.contains(candidate) {
                out.insert(candidate);
            }
        }
        return Some(out);
    }
    let plan = interval_candidates(value, capacity)?;
    let mut out = BTreeSet::new();
    for (mut current, last, step) in plan.pieces {
        loop {
            if value.contains(current) {
                out.insert(current);
            }
            if current == last {
                break;
            }
            current = current.checked_add(step)?;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
