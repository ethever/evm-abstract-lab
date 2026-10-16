//! Mask 与普通区间的精确存在性查询。四个 tight 状态避免指数枚举未知位。
use super::{NumericValue, known_bits::KnownBits};
use alloy_primitives::U256;
use std::collections::BTreeSet;

type Memo = [[[Option<bool>; 2]; 2]; 257];
fn feasible(
    depth: usize,
    lower_tight: bool,
    upper_tight: bool,
    lo: U256,
    hi: U256,
    bits: KnownBits,
    memo: &mut Memo,
) -> bool {
    if depth == 256 {
        return true;
    }
    if let Some(answer) = memo[depth][usize::from(lower_tight)][usize::from(upper_tight)] {
        return answer;
    }
    let index = 255 - depth;
    let answer = (0..=1).any(|digit| {
        let bit = digit != 0;
        if (bits.zero().bit(index) && bit)
            || (bits.one().bit(index) && !bit)
            || (lower_tight && !bit && lo.bit(index))
            || (upper_tight && bit && !hi.bit(index))
        {
            return false;
        }
        feasible(
            depth + 1,
            lower_tight && bit == lo.bit(index),
            upper_tight && bit == hi.bit(index),
            lo,
            hi,
            bits,
            memo,
        )
    });
    memo[depth][usize::from(lower_tight)][usize::from(upper_tight)] = Some(answer);
    answer
}
fn extreme(lo: U256, hi: U256, bits: KnownBits, maximum: bool, memo: &mut Memo) -> Option<U256> {
    if !feasible(0, true, true, lo, hi, bits, memo) {
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
            if feasible(depth + 1, next_lower, next_upper, lo, hi, bits, memo) {
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
    mask_bounds_with_memo(lo, hi, bits, || [[[None; 2]; 2]; 257])
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
    let lower = if lower_matches {
        lo
    } else {
        extreme(lo, hi, bits, false, &mut memo)?
    };
    let upper = if upper_matches {
        hi
    } else {
        extreme(lo, hi, bits, true, &mut memo)?
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
    count: usize,
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
    Some(IntervalCandidates { count, pieces })
}

/// 候选查询真正可能扫描的数量；只计算上界，不枚举或按容量分配。
/// 与完整候选查询共用准入条件，不能用容量截断冒充完整覆盖。
pub(super) fn candidate_visits(value: &NumericValue, capacity: usize) -> usize {
    if let Some(set) = value.constants() {
        return if set.len() <= capacity { set.len() } else { 0 };
    }
    if value.singleton().is_some() {
        return 1;
    }
    bit_pattern_count(value, capacity)
        .unwrap_or_else(|| interval_candidates(value, capacity).map_or(0, |plan| plan.count))
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
mod tests {
    use super::{KnownBits, U256, mask_bounds, mask_bounds_with_memo};

    #[test]
    fn mask_bounds_matches_exhaustive_four_bit_enumeration() {
        for zero in 0_u8..16 {
            for one in 0_u8..16 {
                if zero & one != 0 {
                    continue;
                }
                let bits = KnownBits::new(U256::from(zero), U256::from(one)).unwrap();
                for lo in 0_u8..16 {
                    for hi in lo..16 {
                        let values = (lo..=hi)
                            .filter(|value| value & zero == 0 && value & one == one)
                            .collect::<Vec<_>>();
                        let expected = values
                            .first()
                            .zip(values.last())
                            .map(|(&first, &last)| (U256::from(first), U256::from(last)));
                        assert_eq!(
                            mask_bounds(U256::from(lo), U256::from(hi), bits),
                            expected,
                            "zero={zero} one={one} lo={lo} hi={hi}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn mask_bounds_shortcuts_and_word_boundaries_share_at_most_one_memo() {
        let word = U256::from::<u64>;
        let zero = U256::ZERO;
        let one = word(1);
        let high = one << 255;
        let max = U256::MAX;
        let top = KnownBits::top();
        let even = KnownBits::new(one, zero).unwrap();
        let high_set = KnownBits::new(zero, high).unwrap();
        let high_clear = KnownBits::new(high, zero).unwrap();
        let exact = KnownBits::exact(high);
        let exact_max = KnownBits::exact(max);
        let exact_zero = KnownBits::exact(zero);
        for (lo, hi, bits, expected, memos) in [
            (zero, max, top, Some((zero, max)), 0),
            (one, one, top, Some((one, one)), 0),
            (high, high, exact, Some((high, high)), 0),
            (one, one, even, None, 0),
            (max, zero, top, None, 0),
            (word(2), one, even, None, 0),
            (zero, word(14), even, Some((zero, word(14))), 0),
            (zero, word(15), even, Some((zero, word(14))), 1),
            (one, word(14), even, Some((word(2), word(14))), 1),
            (one, word(15), even, Some((word(2), word(14))), 1),
            (zero, max, high_set, Some((high, max)), 1),
            (high, max, high_set, Some((high, max)), 0),
            (zero, max, high_clear, Some((zero, high - one)), 1),
            (
                high - one,
                high + one,
                high_set,
                Some((high, high + one)),
                1,
            ),
            (zero, max, exact, Some((high, high)), 1),
            (zero, max, exact_max, Some((max, max)), 1),
            (zero, max, exact_zero, Some((zero, zero)), 1),
            (zero, high - one, exact, None, 1),
            (max, max, exact_max, Some((max, max)), 0),
            (max, max, exact_zero, None, 0),
        ] {
            let mut created = 0;
            let actual = mask_bounds_with_memo(lo, hi, bits, || {
                created += 1;
                [[[None; 2]; 2]; 257]
            });
            assert_eq!(actual, expected, "lo={lo} hi={hi} bits={bits:?}");
            assert_eq!(created, memos, "lo={lo} hi={hi} bits={bits:?}");
        }
    }
}
