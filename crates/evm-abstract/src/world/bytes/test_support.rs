//! Assertions shared by conditional and numeric payment tests.

use crate::world::test_support::TestValue;

pub(super) fn total(trace: &[(bool, usize)]) -> usize {
    trace.iter().map(|(_, work)| work).sum()
}

/// A failed atomic payment consumes none of that action's fee.
pub(super) fn paid_prefix(trace: &[(bool, usize)], available: usize) -> &[(bool, usize)] {
    let mut paid = 0;
    let count = trace
        .iter()
        .take_while(|(_, fee)| {
            if *fee > available - paid {
                return false;
            }
            paid += fee;
            true
        })
        .count();
    &trace[..count]
}

// Persistent Eq omits runtime identity. Keep a runtime-only probe so immutable
// input identity cannot mask lost alias information in either payment path.
pub(super) fn assert_complete_value(
    actual: &TestValue,
    expected: &TestValue,
    runtime_probe: &TestValue,
) {
    assert_eq!(actual, expected);
    assert!(runtime_probe.identity().input().is_none());
    assert_eq!(
        actual.same_identity(runtime_probe),
        expected.same_identity(runtime_probe)
    );
}
