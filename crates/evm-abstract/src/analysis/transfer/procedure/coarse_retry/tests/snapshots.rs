//! Common concrete assignments, rather than independent marginal membership.

use super::*;
use crate::domain::{
    AbstractValue,
    symbolic::{ByteStateKind, ExprId, ExprKind, SlotStateKind, SourceId},
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Witness {
    words: BTreeMap<ExprId, U256>,
    cells: BTreeMap<(SourceId, U256), U256>,
}

impl Witness {
    pub(super) fn assign(&mut self, term: &ExprId, value: U256) {
        let cell = match term.kind() {
            ExprKind::Fresh(_) => {
                if let Some(previous) = self.words.insert(term.clone(), value) {
                    assert_eq!(previous, value, "inconsistent common word assignment");
                }
                return;
            }
            ExprKind::Constant(expected) => {
                assert_eq!(*expected, value);
                return;
            }
            ExprKind::SlotRead { state, key } => {
                let SlotStateKind::Input { source, bindings } = state.kind() else {
                    panic!("coarse fixture must have a plain input slot root")
                };
                assert!(bindings.is_empty(), "no stale storage observations");
                (source.clone(), key.as_constant().unwrap())
            }
            ExprKind::ByteRead { state, index } => {
                let ByteStateKind::Input {
                    source,
                    length,
                    bindings,
                } = state.kind()
                else {
                    panic!("coarse fixture must have a plain input byte root")
                };
                assert!(bindings.is_empty(), "no stale byte observations");
                let index = index.as_constant().unwrap();
                assert!(index < length.as_constant().unwrap());
                assert!(value <= U256::from(255));
                (source.clone(), index)
            }
            other => panic!("unsupported joint fixture term {other:?}"),
        };
        if let Some(previous) = self.cells.insert(cell, value) {
            assert_eq!(
                previous, value,
                "current/saved/history sources were conflated"
            );
        }
    }
}

#[test]
fn coarse_entry_has_common_witnesses_for_all_original_joint_tuples_and_rolls_back_versions() {
    let config = fixtures::config();
    let domain = config.domain().unwrap();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let owner = address(0x101);
    let mut input = transfer::initial(&world, &entry, domain).unwrap();
    let saved = domain.join(&word(3), &word(5));
    let current = domain.join(&word(11), &word(13));
    let transient = domain.join(&word(17), &word(19));
    let byte = domain.join(&word(1), &word(2));
    input
        .store
        .write_checked(owner, &word(0), &saved, domain)
        .unwrap();
    input
        .store
        .write_transient_checked(owner, &word(0), &word(7), domain)
        .unwrap();
    input.active_mut().saved_store = input.store.snapshot();
    let history = input.store.read_checked(owner, &word(0), domain).unwrap();
    input
        .store
        .write_checked(owner, &word(0), &current, domain)
        .unwrap();
    input
        .store
        .write_transient_checked(owner, &word(0), &transient, domain)
        .unwrap();
    input.store.touch_account(address(3));
    input
        .active_mut()
        .memory
        .write_byte(&word(0), &byte, 4096, domain)
        .unwrap();
    input.active_mut().stack = vec![history.clone(), history];
    input.normalize();
    let original = form(&input, &entry, domain).unwrap();
    assert!(original.active().stack_aliases.same_class(0, 1));
    let mut budget = WorkBudget::new(config.max_work);
    let (_, bound, live) = CoarseRuntime::prepare(&original, domain, &mut budget).unwrap();
    assert_eq!(live.key().frames, input.key().frames);
    assert_eq!(live.active().program, input.active().program);
    assert_eq!(
        live.active().environment_calldata,
        input.active().environment_calldata
    );
    assert!(!bound.active().stack_aliases.same_class(0, 1));
    assert!(live.relations.is_empty());
    assert_ne!(
        live.active().stack[0].expression(),
        live.active().stack[1].expression()
    );
    let saved_store = live.active().saved_store.state();
    let values = [
        live.store.read_checked(owner, &word(0), domain).unwrap(),
        saved_store.read_checked(owner, &word(0), domain).unwrap(),
        live.active().stack[0].clone(),
        live.active().stack[1].clone(),
        live.store
            .read_transient_checked(owner, &word(0), domain)
            .unwrap(),
        saved_store
            .read_transient_checked(owner, &word(0), domain)
            .unwrap(),
        live.active()
            .memory
            .read_byte_checked(&word(0), domain)
            .unwrap(),
    ];
    assert_ne!(
        live.store.slot_root(owner, false).unwrap(),
        saved_store.slot_root(owner, false).unwrap()
    );
    // Enumerate the complete original finite product. History and saved reads
    // share s in each tuple. ONE assignment simultaneously satisfies every term
    // and every surviving constraint, with no SAT/SMT or marginal-only oracle.
    let mut tuples = 0;
    for s in [3u64, 5] {
        for c in [11, 13] {
            for t in [17, 19] {
                for b in [1, 2] {
                    let expected = [c, s, s, s, t, 7, b].map(U256::from);
                    let mut witness = Witness::default();
                    for (value, expected) in values.iter().zip(expected) {
                        assert!(value.contains(expected));
                        witness.assign(value.expression().unwrap(), expected);
                    }
                    tuples += 1;
                }
            }
        }
    }
    assert_eq!(tuples, 16);
    for kind in [
        OutcomeKind::Return,
        OutcomeKind::Revert,
        OutcomeKind::Failure,
    ] {
        let result = transfer::resume_summary_inner(
            live.clone(),
            kind,
            crate::world::ByteArray::empty(),
            &config,
            domain,
            &mut budget,
            true,
        );
        assert!(result.frontiers.is_empty());
        assert_eq!(result.outcomes.len(), 1);
        let root = result.outcomes[0].store.slot_root(owner, false).unwrap();
        let expected = if kind == OutcomeKind::Return {
            &live.store
        } else {
            saved_store
        };
        assert_eq!(root, expected.slot_root(owner, false).unwrap());
        assert_eq!(
            result.outcomes[0].store.slot_root(owner, true).unwrap(),
            expected.slot_root(owner, true).unwrap()
        );
        assert_eq!(
            result.outcomes[0].store.touched_in_transaction(address(3)),
            Some(true)
        );
    }
    assert_eq!(
        input.active().stack[0].numeric(),
        original.active().stack[0].numeric()
    );
    assert!(!input.active().stack[0].symbolic_limit_reached());
    assert!(input.active().stack[0].contains(U256::from(3)));
    assert!(input.active().stack[0].contains(U256::from(5)));
    assert_ne!(values[0].expression(), values[1].expression());
    assert_eq!(AbstractValue::top().numeric(), values[0].numeric());
}
