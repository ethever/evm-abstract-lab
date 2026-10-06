//! Mask 与普通区间的精确存在性查询。四个 tight 状态避免指数枚举未知位。
use super::{Value, known_bits::KnownBits};
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
fn extreme(lo: U256, hi: U256, bits: KnownBits, maximum: bool) -> Option<U256> {
    let mut memo = [[[None; 2]; 2]; 257];
    if !feasible(0, true, true, lo, hi, bits, &mut memo) {
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
            if feasible(depth + 1, next_lower, next_upper, lo, hi, bits, &mut memo) {
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
    Some((extreme(lo, hi, bits, false)?, extreme(lo, hi, bits, true)?))
}

fn bit_pattern_count(value: &Value, capacity: usize) -> Option<usize> {
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

fn interval_candidates(value: &Value, capacity: usize) -> Option<IntervalCandidates> {
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
pub(super) fn candidate_visits(value: &Value, capacity: usize) -> usize {
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
pub(super) fn candidates(value: &Value, capacity: usize) -> Option<BTreeSet<U256>> {
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
