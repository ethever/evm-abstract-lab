//! Reading-cost proxies complement text savings; none certify equivalence.
//! Wider interfaces require readers to associate more arguments with a body.
use crate::{ProcedurePreference, ReadabilityConfig, ReadabilityProfile, SharedProcedureKind};

pub(super) fn call_overhead(reading: &ReadabilityConfig, words: usize, calls: usize) -> usize {
    let (navigation, associations) = match reading.profile {
        ReadabilityProfile::Linear => (96_usize, 12_usize),
        ReadabilityProfile::Balanced => (32, 4),
        ReadabilityProfile::Compact => (8, 1),
    };
    // Count interface associations separately from the printed argument bytes.
    // These policy units express preferences, not cognitive measurements.
    let pairs = words.saturating_mul(words.saturating_add(1)) / 2;
    navigation
        .saturating_add(pairs.saturating_mul(associations))
        .saturating_mul(calls)
}

pub(super) fn keeps_effects_in_place(
    reading: &ReadabilityConfig,
    kind: SharedProcedureKind,
) -> bool {
    reading.procedures == ProcedurePreference::Auto
        && reading.profile == ReadabilityProfile::Linear
        && kind == SharedProcedureKind::Normal
}

pub(super) fn profitable(
    reading: &ReadabilityConfig,
    source: usize,
    shared: usize,
    words: usize,
    calls: usize,
) -> bool {
    let overhead = if reading.procedures == ProcedurePreference::Shared {
        0
    } else {
        call_overhead(reading, words, calls)
    };
    source > shared.saturating_add(overhead)
}
