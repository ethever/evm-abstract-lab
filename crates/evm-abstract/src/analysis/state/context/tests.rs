//! Recent sources preserve private callers while repeated loop keys stabilize.

use crate::domain::ValueAlgebra;

use super::{canonical, work_size};
use crate::{
    analysis::{machine::fixtures::fixture, transfer},
    domain::ProductAlgebra,
};
use proptest::{collection::vec, prelude::*, test_runner::TestRunner};
use std::collections::BTreeSet;

#[test]
fn canonical_history_keeps_exactly_the_last_occurrence_of_each_source() {
    // Pin the original witnesses as well as checking generated histories. The
    // reference scans suffixes in forward order, independently of the reverse
    // tree traversal used by canonical.
    let check = |history: &[usize]| {
        let expected = history
            .iter()
            .enumerate()
            .filter(|(index, source)| !history[index + 1..].contains(source))
            .map(|(_, source)| *source)
            .collect::<Vec<_>>();
        let actual = canonical(history).unwrap();
        prop_assert_eq!(&actual, &expected, "history {:?}", history);
        prop_assert_eq!(canonical(&actual).unwrap(), actual);
        Ok(())
    };
    for history in [
        vec![],
        vec![1, 2, 3, 2],
        vec![1, 2, 1, 2, 3, 2],
        vec![18, 400],
        vec![18, 400, 448, 416, 256, 400],
        (0..128).collect(),
    ] {
        check(&history).unwrap();
    }
    assert_ne!(
        canonical(&[18, 400]).unwrap(),
        canonical(&[18, 400, 448, 416, 256, 400]).unwrap()
    );
    TestRunner::default()
        .run(&vec(0_usize..512, 0..257), |history| check(&history))
        .unwrap();
}

#[test]
fn repeated_loop_sources_reach_periodic_keys_without_128_iterations() {
    let mut current = Vec::new();
    let mut contexts = BTreeSet::new();
    for source in [12, 13].into_iter().cycle().take(256) {
        current.push(source);
        current = canonical(&current).unwrap();
        contexts.insert(current.clone());
    }
    assert_eq!(
        contexts,
        BTreeSet::from([vec![12], vec![12, 13], vec![13, 12]])
    );
}

#[test]
fn context_work_covers_declared_histories_and_saturates() {
    assert_eq!(work_size(0), 0);
    assert_eq!(work_size(usize::MAX), usize::MAX);
    assert!(work_size(128) >= 128);
}

#[test]
fn known_forms_keep_and_charge_context_while_unknown_forms_erase_it() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    for known in [true, false] {
        let (world, entry) = if known {
            fixture(&[(0x101, "00")])
        } else {
            fixture(&[])
        };
        let mut payload = transfer::initial(&world, &entry, domain).unwrap();
        let baseline = super::super::form(&payload, &entry, domain).unwrap();
        payload.active_mut().key.jump_history = vec![1, 2, 3, 2];
        let formed = super::super::form(&payload, &entry, domain).unwrap();
        if known {
            assert_eq!(formed.active().key.jump_history, [1, 3, 2]);
            assert_eq!(formed.work_size(), baseline.work_size() + 3);
        } else {
            assert!(formed.active().key.jump_history.is_empty());
            assert_eq!(formed.work_size(), baseline.work_size());
        }
    }
}

#[test]
fn normalized_form_preserves_the_supplied_context_without_default_rewriting() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut payload = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
    // An immutable custom policy can deliberately keep repeated callsites.
    // The state layer retains its chosen coordinates rather than applying the
    // default recent-distinct quotient a second time.
    payload.active_mut().key.jump_history = vec![1, 2, 3, 2];
    let summary = super::super::form_normalized(&payload, &entry, algebra.borrowed()).unwrap();
    assert_eq!(summary.active().key.jump_history, [1, 2, 3, 2]);
    let live = super::super::instantiate(&summary, algebra.borrowed()).unwrap();
    assert_eq!(
        live.active().key.jump_history,
        payload.active().key.jump_history
    );
    assert!(super::super::semantic_eq(
        &summary,
        &super::super::form_normalized(&live, &entry, algebra.borrowed()).unwrap(),
    ));
}
