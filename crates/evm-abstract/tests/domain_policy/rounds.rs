use super::{BitIndex, Profile, ReductionStatus, U256, UnaryPredicate, WordBounds, domain};

#[test]
fn freshly_published_bounds_reach_scalar_within_the_first_allowance() {
    let one_round = domain(Profile::Product, 1, 1, 256);
    // This was the old multi-round fixture. Bounds now publishes 32..33 before
    // scalar runs; its existing modulo-12 candidate cover is already {33}.
    let value = one_round
        .from_facts(&[
            UnaryPredicate::UnsignedBounds(
                WordBounds::new(U256::from(10), U256::from(40)).unwrap(),
            ),
            UnaryPredicate::congruent(U256::from(12), U256::from(9)).unwrap(),
            UnaryPredicate::BitClear(BitIndex::new(4).unwrap()),
        ])
        .unwrap();
    assert_eq!(
        value.interval().unsigned_bounds(),
        (U256::from(33), U256::from(33))
    );
    assert_eq!(value.finite_constants().singleton(), Some(U256::from(33)));
    assert_eq!(value.singleton(), Some(U256::from(33)));
    let checked = one_round.reduce(&value);
    assert_eq!(checked.status, ReductionStatus::Stable);
    assert_eq!(checked.rounds, 1);
    assert_eq!(checked.value, value);
    let witnesses: Vec<_> = (10_u64..=40)
        .filter(|x| x % 12 == 9 && x & 16 == 0)
        .collect();
    assert_eq!(witnesses, [33]);
    for raw in 0_u64..=63 {
        assert_eq!(value.contains(U256::from(raw)), witnesses.contains(&raw));
    }
}
