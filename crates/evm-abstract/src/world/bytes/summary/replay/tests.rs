use super::*;
use crate::domain::{
    DomainSpec, FiniteConstantSet, ProductAlgebra, ValueAlgebra,
    relational::RelationLimits,
    symbolic::{ByteStateId, ByteStateKind, ByteView, ExprError, ExprLimits, StateLimits},
};
use std::collections::BTreeSet;

mod carrier;
use carrier::{Policy, RECEIPT, Receipt, Word};

fn policy(max_nodes: usize, max_depth: usize) -> ValueAlgebra<ProductAlgebra> {
    ValueAlgebra::new(ProductAlgebra::from_spec(
        DomainSpec::default().with_relations(RelationLimits {
            max_nodes,
            max_depth,
            ..RelationLimits::default()
        }),
    ))
}

fn array<N: WordElement>(root: ByteStateId) -> EvmByteArray<N> {
    EvmByteArray {
        length: EvmValue::constant(U256::from(64)),
        default: EvmValue::constant(U256::ZERO),
        bytes: BTreeMap::new(),
        memory: true,
        root,
        allocations: BTreeSet::new(),
        operands: BTreeMap::new(),
        conditional: Default::default(),
        failure: None,
    }
}

fn oracle<N: WordElement>(out: &EvmByteArray<N>, limits: ExprLimits) -> Result<(), Error> {
    out.root
        .join_guarded(&out.root, ExprId::constant(U256::ZERO), limits.into())
        .map_err(source)?;
    out.conditional.check(limits).map_err(source)?;
    for value in [&out.length, &out.default]
        .into_iter()
        .chain(out.bytes.values())
        .chain(out.operands.values())
    {
        super::super::super::check_value(value, limits).map_err(source)?;
    }
    for expression in super::super::super::traversal::expressions(&out.root) {
        crate::domain::symbolic::state_check(expression.nodes(), expression.depth(), limits)
            .map_err(source)?;
    }
    Ok(())
}

fn fresh() -> ExprId {
    ExprId::fresh().unwrap()
}

fn roots() -> Vec<ByteStateId> {
    let limits = StateLimits {
        max_nodes: 1_000_000,
        max_depth: 1024,
    };
    let input = ByteStateId::input(SourceId::fresh().unwrap(), fresh());
    let byte = input.write_byte(fresh(), fresh(), limits).unwrap();
    let word = input.write_word(fresh(), fresh(), limits).unwrap();
    let copy = byte
        .copy(
            fresh(),
            ByteView::new(word.clone(), fresh(), fresh()),
            fresh(),
            limits,
        )
        .unwrap();
    let slice = copy.slice(fresh(), fresh(), limits).unwrap();
    let mut shared = slice.join_guarded(&copy, fresh(), limits).unwrap();
    for _ in 0..4 {
        let next = shared.write_word(fresh(), fresh(), limits).unwrap();
        shared = shared.join_guarded(&next, fresh(), limits).unwrap();
    }
    assert!(matches!(shared.kind(), ByteStateKind::Ite { .. }));
    vec![
        ByteStateId::zero(),
        ByteStateId::exact(Arc::from([1, 2, 3])),
        input,
        byte,
        word,
        copy,
        slice,
        shared,
    ]
}

#[test]
fn borrowed_validation_matches_original_order_and_visits_shared_dag_once() {
    for root in roots() {
        let expected = super::super::super::traversal::expressions(&root);
        let mut actual = Vec::new();
        validation::expressions(&root, &mut WorkBudget::new(1_000_000), |expression| {
            actual.push(expression);
            Ok(())
        })
        .unwrap();
        assert_eq!(actual, expected.iter().collect::<Vec<_>>());
        let domain = policy(1_000_000, 1024);
        let out = array::<crate::domain::NumericValue>(root.clone());
        let identity = out.root.id();
        let mut budget = WorkBudget::new(1_000_000);
        assert_eq!(
            check(&out, domain.borrowed(), &mut budget),
            oracle(&out, limits(domain.borrowed()))
        );
        assert_eq!(out.root.id(), identity);
        eprintln!(
            "borrowed validation: root_nodes={} expression_visits={} work={}",
            out.root.nodes(),
            actual.len(),
            budget.used()
        );
        if matches!(out.root.kind(), ByteStateKind::Ite { .. }) {
            assert!(out.root.nodes() > actual.len() * 8);
        }
    }
}

#[test]
fn observation_payload_is_not_visited_but_still_counts_for_root_policy() {
    let domain = policy(100_000, 128);
    let token = SourceId::fresh().unwrap();
    let length = fresh();
    let binding = fresh();
    let mut charges = Vec::new();
    let mut old_work = Vec::new();
    for size in [4usize, 512] {
        let numeric = FiniteConstantSet::try_from_values((0..size).map(U256::from).collect())
            .unwrap()
            .observation();
        let root = ByteStateId::observed(
            token.clone(),
            length.clone(),
            vec![StateBinding {
                index: U256::ZERO,
                value: binding.clone(),
                numeric,
            }],
            limits(domain.borrowed()).into(),
        )
        .unwrap();
        let mut borrowed = Vec::new();
        validation::expressions(&root, &mut WorkBudget::new(100_000), |expression| {
            borrowed.push(expression);
            Ok(())
        })
        .unwrap();
        assert_eq!(borrowed, vec![token.token(), &length, &binding]);
        let out = array::<crate::domain::NumericValue>(root);
        let mut budget = WorkBudget::new(100_000);
        check(&out, domain.borrowed(), &mut budget).unwrap();
        charges.push(budget.used());
        old_work.push(graph_work(out.root.work_size()));
        let small = policy(out.root.nodes() - 1, 128);
        assert_eq!(
            check(&out, small.borrowed(), &mut WorkBudget::new(100_000)),
            oracle(&out, limits(small.borrowed()))
        );
        assert!(matches!(
            oracle(&out, limits(small.borrowed())),
            Err(SummaryError::Source(RangeError::State(
                StateError::Limit { .. }
            )))
        ));
    }
    assert_eq!(charges[0], charges[1]);
    assert!(old_work[1] > old_work[0] * 20);
    eprintln!(
        "observation payload witness: new_work={charges:?} old_root_quote={old_work:?}, 3 borrowed expressions each"
    );
}

#[test]
fn hidden_source_token_and_deep_binding_are_checked_as_expressions() {
    let mut deep = fresh();
    for _ in 0..8 {
        deep = ExprId::operation(
            revm_bytecode::opcode::ADD,
            &[deep, fresh()],
            ExprLimits::default(),
        )
        .unwrap();
    }
    assert!(deep.depth() > 8);
    for hidden_source in [true, false] {
        let root = ByteStateId::observed(
            SourceId::scoped(
                if hidden_source { deep.clone() } else { fresh() },
                U256::ONE,
            ),
            fresh(),
            vec![StateBinding {
                index: U256::from(32),
                value: if hidden_source { fresh() } else { deep.clone() },
                numeric: WordObservation::top(),
            }],
            StateLimits::default(),
        )
        .unwrap();
        let small = ExprLimits {
            max_nodes: 1024,
            max_depth: deep.depth() - 1,
        };
        let mut visits = 0;
        let result = validation::expressions(&root, &mut WorkBudget::new(100_000), |expression| {
            visits += 1;
            crate::domain::symbolic::state_check(expression.nodes(), expression.depth(), small)
                .map_err(source)
        });
        assert_eq!(
            result,
            Err(source(ExprError::Limit {
                nodes: deep.nodes(),
                depth: deep.depth()
            }))
        );
        assert_eq!(visits, if hidden_source { 1 } else { 3 });
    }
}

#[test]
fn cached_whole_root_node_and_depth_rejections_keep_state_error_type() {
    let root = roots().pop().unwrap();
    let out = array::<crate::domain::NumericValue>(root);
    for domain in [
        policy(out.root.nodes() - 1, 1024),
        policy(1_000_000, out.root.depth() - 1),
    ] {
        let expected = oracle(&out, limits(domain.borrowed()));
        assert!(matches!(
            expected,
            Err(SummaryError::Source(RangeError::State(
                StateError::Limit { .. }
            )))
        ));
        assert_eq!(
            check(&out, domain.borrowed(), &mut WorkBudget::new(1_000_000)),
            expected
        );
    }
}

#[test]
fn retained_value_cached_node_and_depth_limits_keep_expression_error_type() {
    let mut deep = fresh();
    for _ in 0..8 {
        deep = ExprId::operation(
            revm_bytecode::opcode::ADD,
            &[deep, fresh()],
            ExprLimits::default(),
        )
        .unwrap();
    }
    for position in 0..4 {
        let mut out = array::<crate::domain::NumericValue>(ByteStateId::zero());
        let value = EvmValue::top().with_expression(deep.clone());
        match position {
            0 => out.length = value,
            1 => out.default = value,
            2 => {
                out.bytes.insert(7, value);
            }
            _ => {
                out.operands.insert(fresh(), value);
            }
        }
        for domain in [policy(deep.nodes() - 1, 64), policy(1024, deep.depth() - 1)] {
            let expected = oracle(&out, limits(domain.borrowed()));
            assert_eq!(
                expected,
                Err(source(ExprError::Limit {
                    nodes: deep.nodes(),
                    depth: deep.depth()
                }))
            );
            assert_eq!(
                check(&out, domain.borrowed(), &mut WorkBudget::new(100_000)),
                expected
            );
        }
    }
}

#[test]
fn every_distinct_hidden_source_is_visited_in_a_shared_copy_root() {
    let first = SourceId::fresh().unwrap();
    let second = SourceId::fresh().unwrap();
    let first_root = ByteStateId::input(first.clone(), fresh());
    let second_root = ByteStateId::input(second.clone(), fresh());
    let root = first_root
        .copy(
            fresh(),
            ByteView::new(second_root, fresh(), fresh()),
            fresh(),
            StateLimits::default(),
        )
        .unwrap()
        .join_guarded(&first_root, fresh(), StateLimits::default())
        .unwrap();
    let mut tokens = Vec::new();
    validation::expressions(&root, &mut WorkBudget::new(100_000), |expression| {
        if expression == first.token() || expression == second.token() {
            tokens.push(expression);
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(tokens.len(), 2);
    assert!(tokens.contains(&first.token()));
    assert!(tokens.contains(&second.token()));
}

#[test]
fn all_value_positions_and_nonempty_conditionals_keep_limit_markers() {
    let domain = policy(1024, 64);
    for position in 0..4 {
        let mut out = array::<crate::domain::NumericValue>(ByteStateId::zero());
        let limited = EvmValue::top().with_symbolic_limit();
        match position {
            0 => out.length = limited,
            1 => out.default = limited,
            2 => {
                out.bytes.insert(7, limited);
            }
            _ => {
                out.operands.insert(fresh(), limited);
            }
        }
        let expected = oracle(&out, limits(domain.borrowed()));
        assert_eq!(
            expected,
            Err(source(ExprError::Limit {
                nodes: usize::MAX,
                depth: usize::MAX
            }))
        );
        assert_eq!(
            check(&out, domain.borrowed(), &mut WorkBudget::new(100_000)),
            expected
        );
    }
    let mut out = array::<crate::domain::NumericValue>(ByteStateId::zero());
    let facts = BTreeMap::from([(fresh(), EvmValue::top().with_expression(fresh()))]);
    let empty = Default::default();
    out.conditional = crate::world::bytes::conditional::ConditionalFacts::join(
        (&facts, &empty),
        (&BTreeMap::new(), &empty),
        &BTreeMap::new(),
        fresh(),
        ExprLimits::default(),
    )
    .unwrap();
    assert!(!out.conditional.has_no_scopes());
    for domain in [domain, policy(1, 1)] {
        assert_eq!(
            check(&out, domain.borrowed(), &mut WorkBudget::new(100_000)),
            oracle(&out, limits(domain.borrowed()))
        );
    }
}

#[test]
fn opaque_native_hooks_are_absent_at_exact_and_refused_budgets() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let mut out = array::<Word>(roots().pop().unwrap());
    let identity = out.root.id();
    let metrics = (out.root.nodes(), out.root.depth());
    out.bytes.insert(
        1,
        EvmValue::from_numeric(Word::window(
            U256::ZERO,
            U256::from(255),
            Some(U256::from(7)),
        )),
    );
    out.operands.insert(
        fresh(),
        EvmValue::from_numeric(Word::window(
            U256::ZERO,
            U256::from(31),
            Some(U256::from(3)),
        ))
        .with_expression(fresh()),
    );
    let expression = fresh();
    let facts = BTreeMap::from([(
        expression.clone(),
        EvmValue::from_numeric(Word::window(
            U256::ZERO,
            U256::from(31),
            Some(U256::from(11)),
        ))
        .with_expression(expression),
    )]);
    let empty = Default::default();
    out.conditional = crate::world::bytes::conditional::ConditionalFacts::join(
        (&facts, &empty),
        (&BTreeMap::new(), &empty),
        &BTreeMap::new(),
        fresh(),
        ExprLimits::default(),
    )
    .unwrap();
    assert!(!out.conditional.has_no_scopes());
    RECEIPT.set(Receipt::default());
    let mut ample = WorkBudget::new(100_000);
    check(&out, domain.borrowed(), &mut ample).unwrap();
    assert_eq!(RECEIPT.get(), Receipt::default());
    let work = ample.used();
    for maximum in 0..=work {
        let mut budget = WorkBudget::new(maximum);
        let result = check(&out, domain.borrowed(), &mut budget);
        assert_eq!(
            result,
            if maximum == work {
                Ok(())
            } else {
                Err(SummaryError::Budget(CloneError::Work))
            }
        );
        assert!(budget.used() <= maximum);
        assert_eq!(RECEIPT.get(), Receipt::default());
        assert_eq!(out.bytes[&1].numeric().excluded, Some(U256::from(7)));
        assert_eq!(
            out.operands.values().next().unwrap().numeric().excluded,
            Some(U256::from(3))
        );
        assert_eq!(out.root.id(), identity);
        assert_eq!((out.root.nodes(), out.root.depth()), metrics);
    }
}

#[test]
fn both_cancellation_owners_stop_before_any_native_hook_or_later_visit() {
    let _scope = RECEIPT.scope();
    let out = array::<Word>(ByteStateId::input(SourceId::fresh().unwrap(), fresh()));
    let domain = ValueAlgebra::new(Policy::default());
    RECEIPT.set(Receipt::default());
    for ledger_owner in [true, false] {
        let token = analysis_control::Cancellation::new();
        let mut budget = if ledger_owner {
            analysis_control::with_cancellation(&token, || WorkBudget::new(100_000))
        } else {
            WorkBudget::new(100_000)
        };
        token.cancel();
        let result = if ledger_owner {
            check(&out, domain.borrowed(), &mut budget)
        } else {
            analysis_control::with_cancellation(&token, || {
                check(&out, domain.borrowed(), &mut budget)
            })
        };
        assert_eq!(result, Err(SummaryError::Budget(CloneError::Cancelled)));
        assert_eq!(budget.used(), 0);
        assert_eq!(RECEIPT.get(), Receipt::default());
        let token = analysis_control::Cancellation::new();
        let mut budget = if ledger_owner {
            analysis_control::with_cancellation(&token, || WorkBudget::new(100_000))
        } else {
            WorkBudget::new(100_000)
        };
        let mut visits = 0;
        let mut run = || {
            validation::expressions(&out.root, &mut budget, |_| {
                visits += 1;
                token.cancel();
                Ok(())
            })
        };
        let result = if ledger_owner {
            run()
        } else {
            analysis_control::with_cancellation(&token, run)
        };
        assert_eq!(result, Err(SummaryError::Budget(CloneError::Cancelled)));
        assert_eq!(visits, 1);
        assert_eq!(RECEIPT.get(), Receipt::default());
    }
}
