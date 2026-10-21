//! The real CALL completion fee must own the entire nonzero COPY path.

use super::*;
use crate::{analysis::transfer::EvmExecution, domain::AlgebraRef};

fn complete<D: EvmNumericAlgebra>(
    payload: EvmMachinePayload<D::Element>,
    source: EvmByteArray<D::Element>,
    config: &ExecutionConfig,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> EvmExecution<D::Element> {
    let mut result = crate::analysis::transfer::empty_execution(payload.clone());
    finish(
        &mut result,
        payload,
        OutcomeKind::Return,
        source,
        &mut crate::analysis::transfer::testing::context(config, budget, domain),
        7,
    );
    result
}

fn check_completion(requested: Value, exact: bool) {
    let algebra = ValueAlgebra::new(Policy);
    let config = ExecutionConfig::default();
    let payload = child(value(0), requested.clone());
    let source = Bytes::exact(&[0x11]);
    let previous = payload
        .call_stack
        .parent()
        .unwrap()
        .memory
        .state_root()
        .clone();
    assert_eq!(requested.singleton_work(), usize::MAX);
    let _queries = QUERIES.scope();
    let _quotes = QUERY_QUOTES.scope();
    let (fee, prepared) = finish_prices(&payload, &source, OutcomeKind::Return, algebra.borrowed());
    assert!(fee > 0 && fee < usize::MAX);
    assert_eq!((QUERIES.get(), QUERY_QUOTES.get()), (0, 0));
    eprintln!("COPY_QUOTE exact={exact} fee={fee} queries=0 quotes=0");
    let mut memory = None;
    for (name, maximum, cancelled) in [
        ("short", fee - 1, false),
        ("full", fee, false),
        ("cancelled", fee, true),
    ] {
        QUERIES.set(0);
        QUERY_QUOTES.set(0);
        let owner = analysis_control::Cancellation::new();
        let mut budget = analysis_control::with_cancellation(&owner, || WorkBudget::new(maximum));
        if cancelled {
            owner.cancel();
        }
        let result = complete(
            payload.clone(),
            source.clone(),
            &config,
            algebra.borrowed(),
            &mut budget,
        );
        let funded = maximum == fee && !cancelled;
        eprintln!(
            "COPY_BODY exact={exact} mode={name} fee={fee} used={} queries={} quotes={} completed={} successors={} frontiers={:?}",
            budget.used(),
            QUERIES.get(),
            QUERY_QUOTES.get(),
            result.completed_calls.len(),
            result.successors.len(),
            result
                .frontiers
                .iter()
                .map(|f| &f.reason)
                .collect::<Vec<_>>()
        );
        assert_eq!((QUERIES.get(), QUERY_QUOTES.get()), (0, 0));
        assert_eq!(result.payload, payload);
        assert_eq!(
            budget.used(),
            if funded {
                fee
            } else if cancelled {
                0
            } else {
                prepared
            }
        );
        if funded {
            assert!(result.frontiers.is_empty());
            assert_eq!(result.completed_calls.len(), 1);
            assert_eq!(result.successors.len(), 1);
            let resumed = result.successors[0].payload.active();
            assert_eq!(resumed.returndata, source);
            assert!(
                matches!(resumed.memory.state_root().kind(), ByteStateKind::Copy { previous: old, .. } if old == &previous)
            );
            memory = Some(resumed.memory.clone());
        } else {
            assert!(result.successors.is_empty());
            assert!(result.completed_calls.is_empty());
            assert_eq!(
                result.frontiers[0].reason,
                if cancelled {
                    FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
                } else {
                    FrontierReason::Work
                }
            );
            assert!(budget.exhausted());
        }
    }
    // Inspect concrete byte witnesses separately from the measured completion.
    drop(_queries);
    drop(_quotes);
    let memory = memory.unwrap();
    let first = memory
        .read_byte_checked(&value(0), algebra.borrowed())
        .unwrap();
    assert!(first.contains(U256::from(0x11)));
    if exact {
        assert_eq!(first.constants().unwrap().first(), Some(&U256::from(0x11)));
    } else {
        assert!(first.contains(U256::from(0xaa)));
    }
    let suffix = memory
        .read_byte_checked(&value(1), algebra.borrowed())
        .unwrap();
    assert!(suffix.contains(U256::from(0xbb)));
    if exact {
        assert_eq!(suffix.constants().unwrap().first(), Some(&U256::from(0xbb)));
    }
}

#[test]
fn finite_nonzero_completion_has_no_opaque_queries_and_preserves_bytes() {
    check_completion(value(1), true);
}

#[test]
fn real_finish_pays_the_census_and_complete_copy_before_publication() {
    use crate::domain::provenance::Origin;
    let algebra = ValueAlgebra::new(Policy);
    let config = ExecutionConfig::default();
    let offset = value(32).with_expression(ExprId::constant(U256::from(32)));
    let requested = value(32).with_expression(ExprId::constant(U256::from(32)));
    let mut payload = child(offset.clone(), requested.clone());
    let nested = payload.call_stack.pop_child().unwrap();
    payload
        .active_mut()
        .memory
        .write_word(
            &value(0),
            &value(7)
                .with_origin(Origin::Storage)
                .with_expression(ExprId::fresh().unwrap()),
            algebra.borrowed(),
        )
        .unwrap();
    payload.call_stack.push_child(nested);
    let mut source = Bytes::memory();
    source
        .write_word(
            &value(0),
            &value(9)
                .with_origin(Origin::Calldata)
                .with_expression(ExprId::fresh().unwrap()),
            algebra.borrowed(),
        )
        .unwrap();
    let parent = &payload.call_stack.parent().unwrap().memory;
    assert!(parent.symbolic_facts().len() > 0 && source.symbolic_facts().len() > 0);
    let first_prefix = payload.relations.constraints().len() + 1;
    let mut expected = parent.clone();
    expected
        .copy_return_data(&offset, &source, &requested, algebra.borrowed())
        .unwrap();
    let _queries = QUERIES.scope();
    let _quotes = QUERY_QUOTES.scope();
    let (total, prepared) =
        finish_prices(&payload, &source, OutcomeKind::Return, algebra.borrowed());
    assert_eq!((QUERIES.get(), QUERY_QUOTES.get()), (0, 0));
    for maximum in [first_prefix - 1, total - 1, total] {
        let mut budget = WorkBudget::new(maximum);
        let result = complete(
            payload.clone(),
            source.clone(),
            &config,
            algebra.borrowed(),
            &mut budget,
        );
        assert_eq!(result.payload, payload);
        assert_eq!((QUERIES.get(), QUERY_QUOTES.get()), (0, 0));
        if maximum == total {
            assert_eq!(budget.used(), total);
            assert!(result.frontiers.is_empty());
            assert_eq!(
                (result.completed_calls.len(), result.successors.len()),
                (1, 1)
            );
            assert_eq!(result.successors[0].payload.active().memory, expected);
            assert_eq!(result.successors[0].payload.active().returndata, source);
        } else {
            assert_eq!(
                budget.used(),
                if maximum < first_prefix { 0 } else { prepared }
            );
            assert!(result.completed_calls.is_empty() && result.successors.is_empty());
            assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
            assert!(budget.exhausted());
        }
    }
}

#[test]
fn unknown_request_completion_has_no_opaque_queries_and_preserves_both_witnesses() {
    check_completion(Value::top(), false);
}

#[test]
fn no_finite_source_length_and_input_need_no_minimum_discovery() {
    use crate::analysis::transfer::query::tests::{
        Policy as OpaquePolicy, RECEIPT, Word as Opaque,
    };

    let algebra = ValueAlgebra::new(OpaquePolicy);
    let config = ExecutionConfig::default();
    let word = |n: u64| EvmValue::<Opaque>::constant(U256::from(n));
    let owner = Address::repeat_byte(0x71);
    let mut world = EvmWorld::new(Fork::Osaka, "COPY opaque source length");
    world
        .insert(
            owner,
            EvmAccount::<Opaque>::from_hex("00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(owner, Address::ZERO, word(0), EvmByteArray::empty());
    let mut parent = initial(&world, &entry, algebra.borrowed()).unwrap();
    parent
        .active_mut()
        .memory
        .copy_from(
            &word(0),
            &EvmByteArray::exact(&[0xaa, 0xbb]),
            &word(0),
            &word(2),
            algebra.borrowed(),
        )
        .unwrap();
    let source = EvmByteArray::<Opaque>::exact(&[0x11]);
    assert!(source.len().constants().is_none());
    assert!(source.len().expression().is_none());
    for retained in [false, true] {
        let requested = if retained {
            EvmValue::<Opaque>::top().with_expression(ExprId::constant(U256::ONE))
        } else {
            word(1)
        };
        assert!(requested.constants().is_none());
        let mut payload = parent.clone();
        let child = parent
            .call_stack
            .root()
            .clone()
            .into_child(EvmContinuation {
                return_block: Some(0),
                output_offset: word(0),
                output_size: requested.clone(),
                creation: None,
            });
        payload.call_stack.push_child(child);
        // The existing opaque carrier's receipt selects one exact value (or
        // nonsingletons) at a time. Cover each scalar class used by this copy.
        for watched in [
            Some(U256::ONE),
            Some(U256::ZERO),
            Some(U256::from(32)),
            None,
        ] {
            let _receipt = RECEIPT.scope();
            RECEIPT.update(|r| {
                r.watched = watched;
                r.fee = usize::MAX;
            });
            let (fee, _) =
                finish_prices(&payload, &source, OutcomeKind::Return, algebra.borrowed());
            assert_eq!((RECEIPT.get().queries, RECEIPT.get().quotes), (0, 0));
            assert!(fee > 0 && fee < usize::MAX);
            RECEIPT.update(|r| {
                r.queries = 0;
                r.quotes = 0;
            });
            let mut budget = WorkBudget::new(fee);
            let result = complete(
                payload.clone(),
                source.clone(),
                &config,
                algebra.borrowed(),
                &mut budget,
            );
            eprintln!(
                "COPY_OPAQUE retained={retained} watched={watched:?} fee={fee} used={} queries={} quotes={} completed={} frontiers={:?}",
                budget.used(),
                RECEIPT.get().queries,
                RECEIPT.get().quotes,
                result.completed_calls.len(),
                result
                    .frontiers
                    .iter()
                    .map(|f| &f.reason)
                    .collect::<Vec<_>>()
            );
            assert_eq!((RECEIPT.get().queries, RECEIPT.get().quotes), (0, 0));
            assert_eq!(budget.used(), fee);
            assert!(result.frontiers.is_empty());
            assert_eq!(result.completed_calls.len(), 1);
            assert_eq!(result.successors.len(), 1);
            let memory = &result.successors[0].payload.active().memory;
            assert!(matches!(
                memory.state_root().kind(),
                ByteStateKind::Copy { source: view, previous, .. }
                    if &view.state == source.state_root()
                        && previous == parent.active().memory.state_root()
            ));
            drop(_receipt);
            let first = memory
                .read_byte_checked(&word(0), algebra.borrowed())
                .unwrap();
            let suffix = memory
                .read_byte_checked(&word(1), algebra.borrowed())
                .unwrap();
            // With no standard numeric views, presentation may stay unknown.
            // The complete captured copy still admits both concrete bytes.
            assert!(first.contains(U256::from(0x11)));
            assert!(suffix.contains(U256::from(0xbb)));
        }
    }
}
