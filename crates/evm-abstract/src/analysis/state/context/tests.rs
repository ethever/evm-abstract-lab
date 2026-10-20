//! Recent sources preserve private callers while repeated loop keys stabilize.

use crate::domain::ValueAlgebra;

use super::{canonical, work_size};
use crate::{
    analysis::{machine::fixtures::fixture, transfer},
    domain::Domain,
};
use std::collections::BTreeSet;

#[test]
fn older_duplicates_move_to_their_most_recent_position() {
    assert_eq!(canonical(&[1, 2, 3, 2]).unwrap(), [1, 3, 2]);
    assert_eq!(canonical(&[1, 2, 1, 2, 3, 2]).unwrap(), [1, 3, 2]);
    assert!(canonical(&[]).unwrap().is_empty());
}

#[test]
fn nested_helpers_keep_the_recent_outer_callsite() {
    let first = [18, 400];
    let second = [18, 400, 448, 416, 256, 400];
    assert_ne!(canonical(&first).unwrap(), canonical(&second).unwrap());
    assert_eq!(canonical(&second).unwrap(), [18, 448, 416, 256, 400]);
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
fn empty_and_declared_bounded_histories_do_not_grow_and_work_saturates() {
    assert!(canonical(&[]).unwrap().is_empty());
    assert_eq!(work_size(0), 0);
    assert_eq!(work_size(usize::MAX), usize::MAX);
    let history = (0..128).collect::<Vec<_>>();
    assert_eq!(canonical(&history).unwrap(), history);
    assert!(work_size(128) >= 128);
}

#[test]
fn known_forms_keep_and_charge_context_while_unknown_forms_erase_it() {
    let domain = Domain::default();
    for known in [true, false] {
        let (world, entry) = if known {
            fixture(&[(0x101, "00")])
        } else {
            fixture(&[])
        };
        let mut payload =
            transfer::initial(&world, &entry, ValueAlgebra::new(domain).borrowed()).unwrap();
        let baseline =
            super::super::form(&payload, &entry, ValueAlgebra::new(domain).borrowed()).unwrap();
        payload.active_mut().key.jump_history = vec![1, 2, 3, 2];
        let formed =
            super::super::form(&payload, &entry, ValueAlgebra::new(domain).borrowed()).unwrap();
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
    let algebra = ValueAlgebra::new(Domain::default());
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
