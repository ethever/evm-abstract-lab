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
            [[[false; 2]; 2]; 257]
        });
        assert_eq!(actual, expected, "lo={lo} hi={hi} bits={bits:?}");
        assert_eq!(created, memos, "lo={lo} hi={hi} bits={bits:?}");
    }
}

#[test]
fn mask_bounds_matches_sparse_full_word_enumeration() {
    // Enumerate complete sets with free bits on both sides of every limb and
    // the sign boundary; no feasibility recurrence is used by this oracle.
    let positions = [0, 63, 64, 127, 128, 191, 192, 255];
    let free = positions
        .iter()
        .fold(U256::ZERO, |mask, bit| mask | (U256::from(1) << *bit));
    for one in [
        U256::ZERO,
        !free,
        U256::from_limbs([0xaaaa_aaaa_aaaa_aaaa; 4]) & !free,
    ] {
        let bits = KnownBits::new(!(free | one), one).unwrap();
        let values = (0_u64..256)
            .map(|pattern| {
                positions.iter().enumerate().fold(one, |value, (i, bit)| {
                    value | (U256::from((pattern >> i) & 1) << *bit)
                })
            })
            .collect::<Vec<_>>();
        let endpoints = [U256::ZERO, U256::MAX]
            .into_iter()
            .chain(values.iter().copied().flat_map(|value| {
                [
                    value.saturating_sub(U256::from(1)),
                    value,
                    value.saturating_add(U256::from(1)),
                ]
            }))
            .collect::<Vec<_>>();
        for (i, &lo) in endpoints.iter().enumerate() {
            for hi in [lo, U256::MAX, endpoints[(i * 37 + 11) % endpoints.len()]] {
                let mut matching = values.iter().copied().filter(|v| lo <= *v && *v <= hi);
                let expected = matching
                    .next()
                    .map(|first| (first, matching.next_back().unwrap_or(first)));
                assert_eq!(
                    mask_bounds(lo, hi, bits),
                    expected,
                    "lo={lo} hi={hi} bits={bits:?}"
                );
            }
        }
    }
}

#[test]
fn mask_bounds_completes_on_a_small_stack() {
    const CHILD: &str = "EVM_ABSTRACT_QUERY_SMALL_STACK";
    if std::env::var_os(CHILD).is_some() {
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| {
                let one = U256::from(1);
                let high = one << 255;
                let bits = KnownBits::new(one, high).unwrap();
                assert_eq!(
                    mask_bounds(one, U256::MAX, bits),
                    Some((high, U256::MAX - one))
                );
            })
            .unwrap()
            .join()
            .unwrap();
        return;
    }

    // Stack overflow aborts a Rust process. Isolate the small-stack worker so
    // a regression is reported as a failed test without aborting the suite.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "domain::query::tests::mask_bounds_completes_on_a_small_stack",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "small-stack query failed: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed;"));
}
