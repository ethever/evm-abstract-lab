//! Prepared Reader actions share the original demand budget.

use super::{Context, Facts, Reader, constant, numeric_domain, packing, scalar_in, zero};
use crate::{
    domain::{
        AbstractValue, Domain, EvmNumericAlgebra, WordElement,
        provenance::Origin,
        symbolic::{
            ByteStateId, ByteStateKind, ExprError, ExprId, ExprLimits, SourceId, StateLimits,
        },
    },
    resource::WorkBudget,
    world::bytes::conditional::{self, ConditionalFacts},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::{num::NonZeroUsize, sync::Arc};

#[test]
fn exact_reader_byte_observations_pay_their_original_operand_quotes() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::fresh().unwrap();
    let value = sourced(1, 9, &key);
    let facts = Facts::from([(key.clone(), value.clone())]);
    let at = ExprId::constant(U256::from(8));
    for word in [false, true] {
        let state = if word {
            ByteStateId::zero()
                .write_word(at.clone(), key.clone(), StateLimits::default())
                .unwrap()
        } else {
            ByteStateId::zero()
                .write_byte(at.clone(), key.clone(), StateLimits::default())
                .unwrap()
        };
        let (index, op, args) = if word {
            (
                ExprId::constant(U256::from(39)),
                opcode::BYTE,
                [constant(31), value.clone()],
            )
        } else {
            (at.clone(), opcode::AND, [value.clone(), constant(255)])
        };
        let expected = domain.observe_byte(op, &args);
        let mut reader = Reader::new(&facts, crate::domain::ValueAlgebra::new(&domain));
        assert_complete(&reader.read(&state, &index), &expected, &value);
        assert_eq!(reader.cache_stats(), (0, 0));
        check(
            state.work_size(),
            &facts,
            &domain,
            &value,
            &[(false, domain.operation_work(&args))],
            |context| Reader::with_context(context.clone(), context.algebra()).read(&state, &index),
        );
    }
}

#[test]
fn stopped_byte_observations_leave_warm_reader_caches_unpublished_and_sticky() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::fresh().unwrap();
    let warm = ExprId::fresh().unwrap();
    let facts = Facts::from([
        (key.clone(), sourced(1, 9, &key)),
        (warm.clone(), AbstractValue::top()),
    ]);
    let frozen = facts.clone();
    let conditional = ConditionalFacts::default();
    let at = ExprId::constant(U256::from(8));
    for word in [false, true] {
        let state = if word {
            ByteStateId::zero()
                .write_word(at.clone(), key.clone(), StateLimits::default())
                .unwrap()
        } else {
            ByteStateId::zero()
                .write_byte(at.clone(), key.clone(), StateLimits::default())
                .unwrap()
        };
        let index = if word {
            ExprId::constant(U256::from(39))
        } else {
            at.clone()
        };
        let preparation = quote(state.work_size(), &facts, domain);
        let mut pilot = WorkBudget::new(usize::MAX);
        let trace;
        {
            let root = Context::budgeted(
                &facts,
                &conditional,
                crate::domain::ValueAlgebra::new(&domain),
                &preparation,
                &mut pilot,
            )
            .unwrap()
            .unwrap();
            let mut reader =
                Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain));
            reader.word_byte_union(&warm);
            assert_ne!(reader.read(&state, &index), AbstractValue::top());
            assert!(!root.stopped());
            assert_eq!(root.reader_calls(), (2, 0));
            trace = root.demand_payments();
        }
        let position = trace.iter().rposition(|(numeric, _)| *numeric).unwrap();
        assert_eq!(position + 1, trace.len());
        for cancel in [false, true] {
            let available = if cancel {
                usize::MAX
            } else {
                total(&trace[..position]) + trace[position].1 - 1
            };
            let mut budget = WorkBudget::new(available);
            let paid;
            {
                let root = Context::budgeted(
                    &facts,
                    &conditional,
                    crate::domain::ValueAlgebra::new(&domain),
                    &preparation,
                    &mut budget,
                )
                .unwrap()
                .unwrap();
                let clone = root.clone();
                let mut reader =
                    Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain));
                assert_ne!(reader.word_byte_union(&warm), AbstractValue::top());
                let cache = reader.cache_stats();
                assert_eq!(cache.0, 1);
                if cancel {
                    let token = analysis_control::Cancellation::new();
                    root.cancel_after_numeric_payments(1, token.clone());
                    analysis_control::with_cancellation(&token, || {
                        assert_eq!(reader.read(&state, &index), AbstractValue::top());
                        assert_eq!(root.check(), Err(ExprError::Cancelled));
                    });
                    assert_eq!(clone.check(), Err(ExprError::Cancelled));
                    assert_eq!(root.demand_payments(), trace);
                } else {
                    assert_eq!(reader.read(&state, &index), AbstractValue::top());
                    assert!(root.work_exhausted());
                    assert_eq!(root.demand_payments(), trace[..position]);
                }
                assert!(!root.cache_refused());
                assert_eq!(root.work_exhausted(), !cancel);
                assert_eq!(root.reader_calls(), (1, 0));
                let payments = root.demand_payments();
                let usage = root.demand_usage();
                assert_eq!(reader.read(&state, &index), AbstractValue::top());
                assert_eq!(reader.word_byte_union(&warm), AbstractValue::top());
                assert_eq!(
                    clone.observe_byte(
                        crate::domain::ValueAlgebra::new(&domain),
                        opcode::AND,
                        &[constant(9), constant(255)]
                    ),
                    AbstractValue::top()
                );
                assert_eq!(root.demand_payments(), payments);
                assert_eq!(root.demand_usage(), usage);
                assert_eq!(root.reader_calls(), (1, 0));
                assert_eq!(reader.cache_stats(), cache);
                paid = total(&payments);
            }
            assert_eq!(budget.used(), paid);
            assert_eq!(budget.exhausted(), !cancel);
        }
    }
    assert_eq!(facts, frozen);
}

fn quote(root_work: usize, facts: &Facts, domain: Domain) -> conditional::Preparation {
    let work = facts.iter().fold(0usize, |work, (key, value)| {
        work + key.work_size() + value.work_size()
    });
    conditional::preparation(
        root_work,
        &zero(),
        work,
        facts.len(),
        &ConditionalFacts::default(),
        crate::domain::ValueAlgebra::new(&domain),
    )
}

fn sourced(lower: usize, upper: usize, key: &ExprId) -> AbstractValue {
    AbstractValue::unsigned_range(U256::from(lower), U256::from(upper))
        .unwrap()
        .with_origin(Origin::Storage)
        .with_expression(key.clone())
        .with_identity(811_036, 9)
        .with_symbolic_limit()
}

fn assert_complete(actual: &AbstractValue, expected: &AbstractValue, probe: &AbstractValue) {
    assert_eq!(actual, expected);
    assert_eq!(actual.numeric(), expected.numeric());
    assert_eq!(actual.finite_constants(), expected.finite_constants());
    assert_eq!(actual.known_bits(), expected.known_bits());
    assert_eq!(actual.interval(), expected.interval());
    assert_eq!(actual.congruence(), expected.congruence());
    assert_eq!(actual.provenance(), expected.provenance());
    assert_eq!(actual.expression(), expected.expression());
    assert_eq!(actual.identity(), expected.identity());
    assert_eq!(
        actual.symbolic_limit_reached(),
        expected.symbolic_limit_reached()
    );
    assert!(probe.identity().input().is_none());
    assert_eq!(actual.same_identity(probe), expected.same_identity(probe));
}

fn total(trace: &[(bool, usize)]) -> usize {
    trace.iter().map(|(_, work)| work).sum()
}

fn numeric(trace: &[(bool, usize)]) -> Vec<usize> {
    trace
        .iter()
        .filter(|(number, _)| *number)
        .map(|(_, work)| *work)
        .collect()
}

fn calls<K: Copy + Into<Option<bool>>>(actions: &[(K, usize)]) -> (usize, usize) {
    (
        actions
            .iter()
            .filter(|(join, _)| (*join).into() == Some(false))
            .count(),
        actions
            .iter()
            .filter(|(join, _)| (*join).into() == Some(true))
            .count(),
    )
}

// Action kinds are independent expected calls: false = apply, true = join.
// None/oracle run the same original algorithm. Real Budget traces pay every
// action; exact per-action boundaries retain the paid prefix without fallback.
fn check(
    root_work: usize,
    facts: &Facts,
    domain: &Domain,
    probe: &AbstractValue,
    actions: &[(bool, usize)],
    action: impl Fn(&Context<'_>) -> AbstractValue,
) {
    let actions: Vec<_> = actions
        .iter()
        .map(|(join, fee)| (Some(*join), *fee))
        .collect();
    check_receipts(root_work, facts, domain, probe, &actions, action);
}

// A pack trial is a numeric payment without a synthetic apply/join call.
fn check_receipts(
    root_work: usize,
    facts: &Facts,
    domain: &Domain,
    probe: &AbstractValue,
    actions: &[(Option<bool>, usize)],
    action: impl Fn(&Context<'_>) -> AbstractValue,
) {
    let conditional = ConditionalFacts::default();
    let frozen = facts.clone();
    let preparation = quote(root_work, facts, *domain);
    let expected = action(&Context::new(
        facts,
        Some(&conditional),
        crate::domain::ValueAlgebra::new(domain),
    ));
    let oracle = Context::prepared(
        facts,
        &conditional,
        crate::domain::ValueAlgebra::new(domain),
        preparation.nodes,
    );
    assert_complete(&action(&oracle), &expected, probe);
    assert!(!oracle.stopped());
    assert!(
        numeric(&oracle.demand_payments()).is_empty(),
        "Oracle owes no Reader fees"
    );
    assert_eq!(oracle.reader_calls(), calls(actions));
    let mut budget = WorkBudget::new(usize::MAX);
    let trace;
    {
        let root = Context::budgeted(
            facts,
            &conditional,
            crate::domain::ValueAlgebra::new(domain),
            &preparation,
            &mut budget,
        )
        .unwrap()
        .unwrap();
        assert_complete(&action(&root), &expected, probe);
        assert!(!root.stopped());
        assert_eq!(root.reader_calls(), calls(actions));
        assert_eq!(
            root.demand_usage(),
            oracle.demand_usage(),
            "Reader fees consume no N"
        );
        trace = root.demand_payments();
        assert_eq!(
            numeric(&trace),
            actions.iter().map(|(_, price)| *price).collect::<Vec<_>>()
        );
        let metadata: Vec<_> = trace
            .iter()
            .filter(|(number, _)| !number)
            .copied()
            .collect();
        assert_eq!(
            &metadata[1..],
            oracle
                .demand_payments()
                .iter()
                .filter(|(number, _)| !number)
                .copied()
                .collect::<Vec<_>>(),
            "only the Budget root prepays its existing fixed lifetime"
        );
    }
    assert_eq!(budget.used(), total(&trace));
    let positions: Vec<_> = trace
        .iter()
        .enumerate()
        .filter(|(_, (number, _))| *number)
        .map(|(index, _)| index)
        .collect();
    let boundaries: Vec<_> = if actions.len() <= 8 {
        (0..actions.len()).collect()
    } else {
        [
            0,
            1,
            actions.len() / 2,
            actions.len() - 2,
            actions.len() - 1,
        ]
        .to_vec()
    };
    for target in boundaries {
        let position = positions[target];
        let prefix = total(&trace[..position]);
        let price = actions[target].1;
        assert!(price > 0);
        for exact in [false, true] {
            let available = prefix + price - usize::from(!exact);
            let mut budget = WorkBudget::new(17 + available);
            assert!(budget.charge(17));
            let paid;
            {
                let root = Context::budgeted(
                    facts,
                    &conditional,
                    crate::domain::ValueAlgebra::new(domain),
                    &preparation,
                    &mut budget,
                )
                .unwrap()
                .unwrap();
                let clone = root.clone();
                let actual = action(&root);
                assert_eq!(
                    root.reader_calls(),
                    calls(&actions[..target + usize::from(exact)])
                );
                paid = root.demand_payments();
                let mut expected_len = 0;
                let mut used = 0;
                for (_, fee) in &trace {
                    if *fee > available - used {
                        break;
                    }
                    used += fee;
                    expected_len += 1;
                }
                assert_eq!(paid, trace[..expected_len]);
                if exact && target + 1 == actions.len() {
                    assert_complete(&actual, &expected, probe);
                    assert!(!root.stopped());
                    assert_eq!(total(&paid), available);
                } else {
                    assert_eq!(
                        actual,
                        AbstractValue::top(),
                        "stopped results are private sentinels"
                    );
                    assert!(root.work_exhausted());
                    assert!(!root.cache_refused());
                    root.check().unwrap();
                    let usage = root.demand_usage();
                    let executed = root.reader_calls();
                    assert_eq!(action(&clone), AbstractValue::top());
                    assert_eq!(
                        clone.apply(
                            crate::domain::ValueAlgebra::new(domain),
                            opcode::ADD,
                            &[constant(1), constant(2)]
                        ),
                        AbstractValue::top()
                    );
                    assert_eq!(
                        clone.join_with(
                            crate::domain::ValueAlgebra::new(domain),
                            &constant(1),
                            &constant(2)
                        ),
                        AbstractValue::top()
                    );
                    assert_eq!(root.demand_payments(), paid);
                    assert_eq!(root.reader_calls(), executed);
                    assert_eq!(root.demand_usage(), usage);
                }
            }
            assert_eq!(budget.used(), 17 + total(&paid));
            assert_eq!(budget.exhausted(), !(exact && target + 1 == actions.len()));
            assert_eq!(facts, &frozen);
        }
    }
}

#[test]
fn scalar_operations_pay_original_input_prices_and_preserve_runtime_alias_semantics() {
    for domain in [
        Domain::default(),
        numeric_domain(Domain::default()),
        Domain::new(NonZeroUsize::new(8).unwrap()),
    ] {
        let left = ExprId::fresh().unwrap();
        let right = ExprId::fresh().unwrap();
        let value = sourced(4, 12, &left);
        let other = value.clone().with_expression(right.clone());
        assert!(value.same_identity(&other));
        let facts = Facts::from([
            (left.clone(), value.clone()),
            (right.clone(), other.clone()),
        ]);
        for op in [opcode::EQ, opcode::ADD] {
            let expression =
                ExprId::operation(op, &[left.clone(), right.clone()], ExprLimits::default())
                    .unwrap();
            let price = domain.operation_work(&[value.clone(), other.clone()]);
            check(
                expression.work_size(),
                &facts,
                &domain,
                &value,
                &[(false, price)],
                |context| {
                    scalar_in(
                        &expression,
                        context,
                        crate::domain::ValueAlgebra::new(&domain),
                    )
                },
            );
        }
    }
}

#[test]
fn scalar_constant_sources_and_unselected_ites_pay_their_distinct_joins() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::constant(U256::from(7));
    let value = constant(7)
        .with_origin(Origin::Storage)
        .with_identity(811_036, 9);
    let facts = Facts::from([(key.clone(), value.clone())]);
    check(
        key.work_size(),
        &facts,
        &domain,
        &value,
        &[(
            true,
            Context::reader_join_work(
                crate::domain::ValueAlgebra::new(&domain),
                &value,
                &constant(7),
            ),
        )],
        |context| scalar_in(&key, context, crate::domain::ValueAlgebra::new(&domain)),
    );

    let yes = ExprId::fresh().unwrap();
    let no = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let left = sourced(1, 12, &yes);
    let right = sourced(4, 16, &no).with_origin(Origin::Calldata);
    assert!(left.same_identity(&right));
    assert!(left.identity().input().is_none());
    assert!(right.identity().input().is_none());
    let expression = ExprId::ite(guard, yes.clone(), no.clone(), ExprLimits::default()).unwrap();
    let facts = Facts::from([(yes, left.clone()), (no, right.clone())]);
    let expected = scalar_in(
        &expression,
        &Context::new(&facts, None, crate::domain::ValueAlgebra::new(&domain)),
        crate::domain::ValueAlgebra::new(&domain),
    );
    assert!(
        !expected.same_identity(&left) && !expected.same_identity(&right),
        "the unresolved scalar join drops runtime-only identity"
    );
    check(
        expression.work_size(),
        &facts,
        &domain,
        &left,
        &[(
            true,
            Context::reader_join_work(crate::domain::ValueAlgebra::new(&domain), &left, &right),
        )],
        |context| {
            scalar_in(
                &expression,
                context,
                crate::domain::ValueAlgebra::new(&domain),
            )
        },
    );
}

#[test]
fn byte_mask_extraction_and_finite_index_joins_have_one_actual_owner() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::fresh().unwrap();
    let value = sourced(0x100, 0x1ff, &key);
    let facts = Facts::from([(key.clone(), value.clone())]);
    let at = ExprId::constant(U256::ZERO);
    let byte = ByteStateId::zero()
        .write_byte(at.clone(), key.clone(), StateLimits::default())
        .unwrap();
    check(
        byte.work_size(),
        &facts,
        &domain,
        &value,
        &[(
            false,
            domain.operation_work(&[value.clone(), constant(255)]),
        )],
        |context| Reader::with_context(context.clone(), context.algebra()).read(&byte, &at),
    );
    let word = ByteStateId::zero()
        .write_word(at, key, StateLimits::default())
        .unwrap();
    let last = ExprId::constant(U256::from(31));
    check(
        word.work_size(),
        &facts,
        &domain,
        &value,
        &[(false, domain.operation_work(&[constant(31), value.clone()]))],
        |context| Reader::with_context(context.clone(), context.algebra()).read(&word, &last),
    );

    let index = ExprId::fresh().unwrap();
    let facts = Facts::from([(index.clone(), domain.join(&zero(), &constant(1)))]);
    let exact = ByteStateId::exact(Arc::from([5_u8, 7]));
    check(
        exact.work_size(),
        &facts,
        &domain,
        &value,
        &[(
            true,
            Context::reader_join_work(
                crate::domain::ValueAlgebra::new(&domain),
                &constant(5),
                &constant(7),
            ),
        )],
        |context| Reader::with_context(context.clone(), context.algebra()).read(&exact, &index),
    );
}

#[test]
fn whole_word_ite_joins_once_after_complete_branch_packing() {
    let domain = numeric_domain(Domain::default());
    let yes = ByteStateId::exact(Arc::from([5_u8; 32]));
    let no = ByteStateId::exact(Arc::from([7_u8; 32]));
    let state = yes
        .join_guarded(&no, ExprId::fresh().unwrap(), StateLimits::default())
        .unwrap();
    assert!(matches!(state.kind(), ByteStateKind::Ite { .. }));
    let empty = Facts::new();
    let unmetered = Context::new(&empty, None, crate::domain::ValueAlgebra::new(&domain));
    let left = packing::pack(
        &vec![constant(5); 32],
        crate::domain::ValueAlgebra::new(&domain),
        &unmetered,
    )
    .unwrap();
    let right = packing::pack(
        &vec![constant(7); 32],
        crate::domain::ValueAlgebra::new(&domain),
        &unmetered,
    )
    .unwrap();
    let probe = sourced(1, 9, &ExprId::fresh().unwrap());
    let five = crate::domain::NumericValue::constant(U256::from(5));
    let seven = crate::domain::NumericValue::constant(U256::from(7));
    check_receipts(
        state.work_size(),
        &Facts::new(),
        &domain,
        &probe,
        &[
            (None, domain.pack_word_work(&[&five; 32]).unwrap()),
            (None, left.singleton_work()),
            (None, domain.pack_word_work(&[&seven; 32]).unwrap()),
            (None, right.singleton_work()),
            (
                Some(true),
                Context::reader_join_work(crate::domain::ValueAlgebra::new(&domain), &left, &right),
            ),
        ],
        |context| {
            Reader::with_context(context.clone(), context.algebra())
                .word(&state, &ExprId::constant(U256::ZERO))
        },
    );
}

// Independent old BYTE loop supplies each original operand-dependent price.
fn union_actions(value: &AbstractValue, domain: Domain) -> Vec<(bool, usize)> {
    let mut out = None;
    let mut actions = Vec::new();
    for index in 0..32 {
        let args = [constant(index), value.clone()];
        actions.push((false, domain.operation_work(&args)));
        let byte = domain.apply(opcode::BYTE, &args);
        out = Some(match out {
            Some(old) => {
                actions.push((
                    true,
                    Context::reader_join_work(
                        crate::domain::ValueAlgebra::new(&domain),
                        &old,
                        &byte,
                    ),
                ));
                domain.join(&old, &byte)
            }
            None => byte,
        });
    }
    actions
}

#[test]
fn byte_unions_pay_executed_calls_and_complete_warm_hits_owe_no_new_actions() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::fresh().unwrap();
    let probe = sourced(1, 9, &key);
    for value in [
        AbstractValue::top()
            .with_origin(Origin::Storage)
            .with_identity(811_036, 9),
        constant(0x1122)
            .with_origin(Origin::Calldata)
            .with_identity(811_036, 9),
    ] {
        let facts = Facts::from([(key.clone(), value.clone())]);
        let actions = if value.numeric().is_top() {
            vec![(false, domain.operation_work(&[zero(), value.clone()]))]
        } else {
            union_actions(&value, domain)
        };
        check(
            key.work_size(),
            &facts,
            &domain,
            &probe,
            &actions,
            |context| {
                Reader::with_context(context.clone(), context.algebra()).word_byte_union(&key)
            },
        );
        let conditional = ConditionalFacts::default();
        let preparation = quote(key.work_size(), &facts, domain);
        let mut budget = WorkBudget::new(usize::MAX);
        let root = Context::budgeted(
            &facts,
            &conditional,
            crate::domain::ValueAlgebra::new(&domain),
            &preparation,
            &mut budget,
        )
        .unwrap()
        .unwrap();
        let mut reader =
            Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain));
        let expected = reader.word_byte_union(&key);
        assert_eq!(reader.word_bytes.len(), 1);
        let trace = root.demand_payments();
        let used = reader.retained_work;
        assert_complete(&reader.word_byte_union(&key), &expected, &probe);
        assert_eq!(root.demand_payments(), trace);
        assert_eq!(root.reader_calls(), calls(&actions));
        assert_eq!(reader.retained_work, used);
    }
}

#[test]
fn certified_byte_image_pays_fixed_proof_then_one_transfer_and_no_warm_fee() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::fresh().unwrap();
    let value = AbstractValue::unknown_address()
        .with_expression(key.clone())
        .with_identity(811_037, 10)
        .with_symbolic_limit();
    let facts = Facts::from([(key.clone(), value.clone())]);
    let frozen = facts.clone();
    let expected =
        Reader::new(&facts, crate::domain::ValueAlgebra::new(&domain)).word_byte_union(&key);
    let conditional = ConditionalFacts::default();
    let preparation = quote(key.work_size(), &facts, domain);
    let transfer = domain.operation_work(&[
        zero(),
        value
            .clone()
            .with_numeric(crate::domain::NumericValue::top()),
    ]);
    let oracle = Context::prepared(
        &facts,
        &conditional,
        crate::domain::ValueAlgebra::new(&domain),
        preparation.nodes,
    );
    assert_complete(
        &Reader::with_context(oracle.clone(), crate::domain::ValueAlgebra::new(&domain))
            .word_byte_union(&key),
        &expected,
        &value,
    );
    assert!(numeric(&oracle.demand_payments()).is_empty());
    assert_eq!(oracle.reader_calls(), (1, 0));
    let mut probe = WorkBudget::new(usize::MAX);
    let full;
    {
        let root = Context::budgeted(
            &facts,
            &conditional,
            crate::domain::ValueAlgebra::new(&domain),
            &preparation,
            &mut probe,
        )
        .unwrap()
        .unwrap();
        let mut reader =
            Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain));
        assert_complete(&reader.word_byte_union(&key), &expected, &value);
        assert_eq!(root.reader_calls(), (1, 0));
        assert_eq!(root.demand_usage(), oracle.demand_usage());
        full = root.demand_payments();
        assert_eq!(numeric(&full), [32 * 256, transfer]);
        let retained = reader.retained_work;
        assert_complete(&reader.word_byte_union(&key), &expected, &value);
        assert_eq!(root.demand_payments(), full);
        assert_eq!(root.reader_calls(), (1, 0));
        assert_eq!(reader.retained_work, retained);
        assert_eq!(reader.word_bytes.len(), 1);
    }
    assert_eq!(probe.used(), total(&full));
    let positions: Vec<_> = full
        .iter()
        .enumerate()
        .filter(|(_, (number, _))| *number)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(positions.len(), 2);
    for (stage, &position) in positions.iter().enumerate() {
        for exact in [false, true] {
            let available = total(&full[..position]) + full[position].1 - usize::from(!exact);
            let mut budget = WorkBudget::new(available);
            {
                let root = Context::budgeted(
                    &facts,
                    &conditional,
                    crate::domain::ValueAlgebra::new(&domain),
                    &preparation,
                    &mut budget,
                )
                .unwrap()
                .unwrap();
                let clone = root.clone();
                let mut reader =
                    Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain));
                let actual = reader.word_byte_union(&key);
                assert_eq!(
                    root.demand_payments(),
                    full[..position + usize::from(exact)]
                );
                let complete = stage == 1 && exact;
                assert_eq!(root.reader_calls(), (usize::from(complete), 0));
                if complete {
                    assert_complete(&actual, &expected, &value);
                    assert!(!root.stopped());
                    assert_eq!(reader.word_bytes.len(), 1);
                } else {
                    assert_eq!(actual, AbstractValue::top());
                    assert!(root.work_exhausted());
                    assert!(!root.cache_refused());
                    assert!(reader.word_bytes.is_empty());
                    assert_eq!(reader.retained_work, 0);
                    let trace = root.demand_payments();
                    let usage = root.demand_usage();
                    assert!(!clone.pay_byte_image(crate::domain::ValueAlgebra::new(&domain)));
                    assert_eq!(reader.word_byte_union(&key), AbstractValue::top());
                    assert_eq!(root.demand_payments(), trace);
                    assert_eq!(root.demand_usage(), usage);
                    assert_eq!(root.reader_calls(), (0, 0));
                }
            }
            assert_eq!(budget.used(), total(&full[..position + usize::from(exact)]));
        }
    }
    assert_eq!(facts, frozen);
}

#[test]
fn certified_byte_image_cancellation_and_full_policy_guard_precede_publication() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), AbstractValue::unknown_address())]);
    let frozen = facts.clone();
    let conditional = ConditionalFacts::default();
    let preparation = quote(key.work_size(), &facts, domain);
    // Stage one is the proof prepayment; stage two is ordinary BYTE payment.
    for stage in [1, 2] {
        let token = analysis_control::Cancellation::new();
        let mut budget = WorkBudget::new(usize::MAX);
        let paid;
        {
            let root = Context::budgeted(
                &facts,
                &conditional,
                crate::domain::ValueAlgebra::new(&domain),
                &preparation,
                &mut budget,
            )
            .unwrap()
            .unwrap();
            root.cancel_after_numeric_payments(stage, token.clone());
            let clone = root.clone();
            let mut reader =
                Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain));
            analysis_control::with_cancellation(&token, || {
                assert_eq!(reader.word_byte_union(&key), AbstractValue::top());
                assert_eq!(root.check(), Err(ExprError::Cancelled));
            });
            assert_eq!(clone.check(), Err(ExprError::Cancelled));
            assert_eq!(root.reader_calls(), (0, 0));
            assert_eq!(numeric(&root.demand_payments()).len(), stage);
            assert!(reader.word_bytes.is_empty());
            assert_eq!(reader.retained_work, 0);
            assert!(!root.work_exhausted());
            assert!(!root.cache_refused());
            let trace = root.demand_payments();
            assert!(!clone.pay_byte_image(crate::domain::ValueAlgebra::new(&domain)));
            assert_eq!(reader.word_byte_union(&key), AbstractValue::top());
            assert_eq!(root.demand_payments(), trace);
            paid = total(&trace);
        }
        assert_eq!(budget.used(), paid);
        assert!(!budget.exhausted());
    }
    let mut budget = WorkBudget::new(usize::MAX);
    {
        let root = Context::budgeted(
            &facts,
            &conditional,
            crate::domain::ValueAlgebra::new(&domain),
            &preparation,
            &mut budget,
        )
        .unwrap()
        .unwrap();
        let trace = root.demand_payments();
        let changed = Domain::from_spec(
            crate::domain::DomainSpec::new(
                crate::domain::Profile::Product,
                NonZeroUsize::new(domain.capacity()).unwrap(),
                NonZeroUsize::new(domain.spec().reduction_rounds()).unwrap(),
                NonZeroUsize::new(domain.spec().fact_limit() + 1).unwrap(),
            )
            .with_relations(domain.spec().relations()),
        );
        assert!(!root.pay_byte_image(crate::domain::ValueAlgebra::new(&changed)));
        assert!(root.cache_refused());
        assert!(!root.work_exhausted());
        assert_eq!(root.demand_payments(), trace);
        assert_eq!(root.reader_calls(), (0, 0));
    }
    assert_eq!(facts, frozen);
}

#[test]
fn successful_packing_has_no_synthetic_transfers_and_fallback_pays_all_original_shl_or_calls() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::fresh().unwrap();
    let value = sourced(1, 3, &key);
    let facts = Facts::from([(key.clone(), value.clone())]);
    let at = ExprId::constant(U256::ZERO);
    let state = ByteStateId::zero()
        .write_byte(at.clone(), key, StateLimits::default())
        .unwrap();
    let mut reference = Reader::new(&facts, crate::domain::ValueAlgebra::new(&domain));
    let bytes: Vec<_> = (0..32)
        .map(|index| reference.read(&state, &ExprId::constant(U256::from(index))))
        .collect();
    let unmetered = Context::new(&facts, None, crate::domain::ValueAlgebra::new(&domain));
    assert!(
        packing::pack(
            &bytes,
            crate::domain::ValueAlgebra::new(&domain),
            &unmetered
        )
        .is_none()
    );
    let pack_fee = domain
        .pack_word_work(&std::array::from_fn(|i| bytes[i].numeric()))
        .unwrap();
    let mut actions = vec![(
        false,
        domain.operation_work(&[value.clone(), constant(255)]),
    )];
    let mut parts: Vec<_> = bytes
        .into_iter()
        .enumerate()
        .map(|(index, byte)| {
            let args = [constant((31 - index) * 8), byte];
            actions.push((false, domain.operation_work(&args)));
            domain.apply(opcode::SHL, &args)
        })
        .collect();
    while parts.len() > 1 {
        parts = parts
            .chunks(2)
            .map(|pair| {
                actions.push((false, domain.operation_work(pair)));
                domain.apply(opcode::OR, pair)
            })
            .collect();
    }
    assert_eq!(actions.len(), 1 + 32 + 31);
    let mut receipts: Vec<_> = actions
        .iter()
        .map(|(join, fee)| (Some(*join), *fee))
        .collect();
    receipts.insert(1, (None, pack_fee));
    check_receipts(
        state.work_size(),
        &facts,
        &domain,
        &value,
        &receipts,
        |context| Reader::with_context(context.clone(), context.algebra()).word(&state, &at),
    );

    let exact = ByteStateId::exact(Arc::from([7_u8; 32]));
    let seven = crate::domain::NumericValue::constant(U256::from(7));
    check_receipts(
        exact.work_size(),
        &Facts::new(),
        &domain,
        &value,
        &[
            (None, domain.pack_word_work(&[&seven; 32]).unwrap()),
            (
                None,
                crate::domain::NumericValue::constant(U256::from_be_bytes([7_u8; 32]))
                    .singleton_work(),
            ),
        ],
        |context| Reader::with_context(context.clone(), context.algebra()).word(&exact, &at),
    );
    let unknown = ByteStateId::input(SourceId::fresh().unwrap(), ExprId::constant(U256::from(32)));
    let byte = crate::domain::NumericValue::unknown_byte();
    check_receipts(
        unknown.work_size(),
        &Facts::new(),
        &domain,
        &value,
        &[
            (None, domain.pack_word_work(&[&byte; 32]).unwrap()),
            (None, crate::domain::NumericValue::top().singleton_work()),
        ],
        |context| Reader::with_context(context.clone(), context.algebra()).word(&unknown, &at),
    );
}

#[test]
fn cancellation_at_a_reader_payment_is_typed_sticky_and_never_publishes_a_union() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::fresh().unwrap();
    let warm = ExprId::fresh().unwrap();
    let value = constant(0x1122)
        .with_origin(Origin::Storage)
        .with_identity(811_036, 9);
    let facts = Facts::from([
        (key.clone(), value.clone()),
        (warm.clone(), AbstractValue::top()),
    ]);
    let frozen = facts.clone();
    let conditional = ConditionalFacts::default();
    let preparation = quote(key.work_size(), &facts, domain);
    let expected_actions = union_actions(&value, domain);
    // Payment 1 funds BYTE(0), payment 2 funds BYTE(1), payment 3
    // attempts their first union join. Metadata cannot trigger this hook.
    for (stage, expected_calls) in [(1, (0, 0)), (3, (2, 0))] {
        let token = analysis_control::Cancellation::new();
        let mut budget = WorkBudget::new(usize::MAX);
        let paid;
        {
            let root = Context::budgeted(
                &facts,
                &conditional,
                crate::domain::ValueAlgebra::new(&domain),
                &preparation,
                &mut budget,
            )
            .unwrap()
            .unwrap();
            let clone = root.clone();
            let child = root.branch(&ExprId::fresh().unwrap(), true);
            let mut reader =
                Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain));
            let mut other =
                Reader::with_context(clone.clone(), crate::domain::ValueAlgebra::new(&domain));
            other.word_byte_union(&warm);
            assert_eq!(other.word_bytes.len(), 1);
            let before_calls = root.reader_calls();
            let before_numeric = numeric(&root.demand_payments()).len();
            root.cancel_after_numeric_payments(stage, token.clone());
            analysis_control::with_cancellation(&token, || {
                assert_eq!(reader.word_byte_union(&key), AbstractValue::top());
                assert_eq!(root.check(), Err(ExprError::Cancelled));
            });
            assert!(!analysis_control::current_cancellation().is_cancelled());
            assert_eq!(
                root.reader_calls(),
                (
                    before_calls.0 + expected_calls.0,
                    before_calls.1 + expected_calls.1
                )
            );
            assert_eq!(
                numeric(&root.demand_payments()).len(),
                before_numeric + stage
            );
            assert_eq!(
                numeric(&root.demand_payments())[before_numeric..],
                expected_actions[..stage]
                    .iter()
                    .map(|(_, price)| *price)
                    .collect::<Vec<_>>(),
                "the hook reaches the stated Reader BYTE/join stage"
            );
            assert!(reader.word_bytes.is_empty());
            assert_eq!(reader.retained_work, 0);
            let retained = other.retained_work;
            let trace = root.demand_payments();
            let usage = root.demand_usage();
            for context in [&root, &clone, &child] {
                assert_eq!(context.check(), Err(ExprError::Cancelled));
                assert!(!context.work_exhausted());
                assert!(!context.cache_refused());
                assert_eq!(
                    scalar_in(&key, context, crate::domain::ValueAlgebra::new(&domain)),
                    AbstractValue::top()
                );
            }
            assert_eq!(
                other.word_byte_union(&warm),
                AbstractValue::top(),
                "a warm clone observes the same sticky stop"
            );
            assert_eq!(reader.word_byte_union(&key), AbstractValue::top());
            assert_eq!(root.demand_payments(), trace);
            assert_eq!(root.demand_usage(), usage);
            assert_eq!(other.retained_work, retained);
            assert_eq!(other.word_bytes.len(), 1);
            paid = total(&trace);
        }
        assert_eq!(budget.used(), paid);
        assert!(
            !budget.exhausted(),
            "cancellation is sticky in the Demand after dynamic scope exit"
        );
        assert_eq!(facts, frozen);
    }
}

#[test]
fn unpaid_union_byte_and_join_leave_cold_and_warm_caches_stopped_without_new_charges() {
    let domain = numeric_domain(Domain::default());
    let key = ExprId::fresh().unwrap();
    let warm = ExprId::fresh().unwrap();
    let value = constant(0x1122).with_origin(Origin::Storage);
    let facts = Facts::from([(key.clone(), value), (warm.clone(), AbstractValue::top())]);
    let frozen = facts.clone();
    let conditional = ConditionalFacts::default();
    let preparation = quote(key.work_size(), &facts, domain);
    let mut probe = WorkBudget::new(usize::MAX);
    let full;
    {
        let root = Context::budgeted(
            &facts,
            &conditional,
            crate::domain::ValueAlgebra::new(&domain),
            &preparation,
            &mut probe,
        )
        .unwrap()
        .unwrap();
        Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain))
            .word_byte_union(&warm);
        Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain))
            .word_byte_union(&key);
        assert!(!root.stopped());
        full = root.demand_payments();
    }
    assert_eq!(probe.used(), total(&full));
    let positions: Vec<_> = full
        .iter()
        .enumerate()
        .filter(|(_, (number, _))| *number)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(positions.len(), 1 + 32 + 31);
    for numeric_index in [1, 3, positions.len() - 1] {
        let position = positions[numeric_index];
        let prefix = total(&full[..position]);
        let price = full[position].1;
        let mut budget = WorkBudget::new(prefix + price - 1);
        {
            let root = Context::budgeted(
                &facts,
                &conditional,
                crate::domain::ValueAlgebra::new(&domain),
                &preparation,
                &mut budget,
            )
            .unwrap()
            .unwrap();
            let clone = root.clone();
            let mut other = Reader::with_context(clone, crate::domain::ValueAlgebra::new(&domain));
            other.word_byte_union(&warm);
            let retained = other.retained_work;
            assert_eq!(other.word_bytes.len(), 1);
            let mut reader =
                Reader::with_context(root.clone(), crate::domain::ValueAlgebra::new(&domain));
            assert_eq!(reader.word_byte_union(&key), AbstractValue::top());
            assert!(root.work_exhausted());
            assert!(!root.cache_refused());
            assert!(reader.word_bytes.is_empty());
            assert_eq!(reader.retained_work, 0);
            assert_eq!(root.demand_payments(), full[..position]);
            let executed = root.reader_calls();
            let usage = root.demand_usage();
            assert_eq!(other.word_byte_union(&warm), AbstractValue::top());
            assert_eq!(reader.word_byte_union(&key), AbstractValue::top());
            assert_eq!(root.reader_calls(), executed);
            assert_eq!(root.demand_payments(), full[..position]);
            assert_eq!(root.demand_usage(), usage);
            assert_eq!(other.retained_work, retained);
            assert_eq!(other.word_bytes.len(), 1);
        }
        assert_eq!(budget.used(), prefix);
        assert!(budget.exhausted());
        assert_eq!(facts, frozen);
    }
}
