use super::*;
use crate::{
    domain::{identity::Symbol, provenance::Origin},
    world::bytes::{ByteArray, constant, expr, limits},
};
use alloy_primitives::U256;
use std::num::NonZeroUsize;

// The oracle retains the original ordered public-to-this-module operation;
// it does not duplicate value cleanup or a metadata encoding.
fn reference(
    source: &BTreeMap<ExprId, AbstractValue>,
    domain: Domain,
    limits: ExprLimits,
) -> Result<BTreeMap<ExprId, AbstractValue>, ExprError> {
    let mut facts = BTreeMap::new();
    for (expression, value) in source {
        remember(&mut facts, expression.clone(), value, domain, limits)?;
    }
    Ok(facts)
}

fn compare(
    source: &BTreeMap<ExprId, AbstractValue>,
    domain: Domain,
    limits: ExprLimits,
) -> Result<BTreeMap<ExprId, AbstractValue>, ExprError> {
    let before = source.clone();
    let actual = import_empty(source, domain, limits);
    assert_eq!(actual, reference(source, domain, limits));
    assert_eq!(*source, before);
    actual
}

#[test]
fn empty_import_cleans_literals_and_retains_nonliteral_guarantees() {
    let domain = Domain::default();
    let input = ExprId::input(17, Symbol::CalldataLength);
    let observed = domain
        .join(&constant(7), &constant(8))
        .with_symbol(Symbol::CalldataLength, Some(17))
        .with_origin(Origin::Calldata)
        .with_identity(19, 3);
    let temporary = constant(9).with_origin(Origin::Memory).with_identity(19, 4);
    let symbolic = ExprId::fresh().unwrap();
    let runtime = ExprId::fresh().unwrap();
    assert!(observed.identity().input().is_some());
    assert!(temporary.same_identity(&temporary));
    let source = BTreeMap::from([
        (ExprId::constant(U256::ZERO), constant(0)),
        // Neither the source number nor its input occurrence may replace the
        // literal key's exact proof. Joining also removes its must-alias term.
        (ExprId::constant(U256::ONE), observed.clone()),
        (symbolic.clone(), observed.clone()),
        (runtime.clone(), temporary.clone()),
    ]);
    for current in [
        domain,
        Domain::new(NonZeroUsize::new(1).unwrap()),
        Domain::from_spec(domain.spec().with_relations(
            crate::domain::relational::RelationLimits {
                max_nodes: 64,
                max_depth: 16,
                ..domain.spec().relations()
            },
        )),
    ] {
        let imported = compare(&source, current, limits(current)).unwrap();
        assert!(!imported.contains_key(&ExprId::constant(U256::ZERO)));
        let mut expected = observed.clone();
        expected.forget_identity();
        assert_eq!(imported[&symbolic], expected);
        assert_eq!(imported[&symbolic].expression(), Some(&input));
        assert_eq!(imported[&symbolic].numeric(), observed.numeric());
        assert_eq!(imported[&symbolic].provenance(), observed.provenance());
        assert!(imported[&symbolic].same_identity(&observed));
        // Runtime identity is intentionally excluded from AbstractValue Eq.
        assert!(!imported[&runtime].same_identity(&temporary));
        assert_eq!(imported[&runtime].numeric(), temporary.numeric());
        assert_eq!(imported[&runtime].provenance(), temporary.provenance());
        let literal = &imported[&ExprId::constant(U256::ONE)];
        assert_eq!(literal.singleton(), Some(U256::ONE));
        assert_eq!(literal.expression(), None);
        assert!(literal.identity().input().is_none());
        assert!(!literal.same_identity(&observed));
        assert_eq!(literal.provenance(), observed.provenance());
    }
}

#[test]
fn empty_import_counts_only_retained_processed_entries() {
    let domain = Domain::default();
    let mut source: BTreeMap<_, _> = (0..16)
        .map(|n| (ExprId::constant(U256::from(n)), constant(n)))
        .collect();
    let retained = ExprId::fresh().unwrap();
    source.insert(retained.clone(), constant(9));
    let policy = ExprLimits {
        max_nodes: 1,
        ..limits(domain)
    };
    assert_eq!(
        compare(&source, domain, policy).unwrap(),
        BTreeMap::from([(retained, constant(9))])
    );

    let mut source: BTreeMap<_, _> = (0..3)
        .map(|_| (ExprId::fresh().unwrap(), constant(1)))
        .collect();
    // The third value must not replace the preceding insertion-count error.
    *source.values_mut().nth(2).unwrap() = constant(1).with_symbolic_limit();
    assert_eq!(
        compare(&source, domain, policy),
        Err(ExprError::Limit { nodes: 2, depth: 1 })
    );
    // Value validation precedes insertion and its resulting count check.
    *source.values_mut().nth(1).unwrap() = constant(1).with_symbolic_limit();
    assert_eq!(
        compare(&source, domain, policy),
        Err(ExprError::Limit {
            nodes: usize::MAX,
            depth: usize::MAX,
        })
    );
}

#[test]
fn empty_import_preserves_current_value_key_and_cancellation_priority() {
    let domain = Domain::default();
    let complex = expr(&AbstractValue::unknown_byte()).unwrap();
    assert!(complex.nodes() > 1);
    let sentinel = ExprError::Limit {
        nodes: usize::MAX,
        depth: usize::MAX,
    };
    let source = BTreeMap::from([(complex.clone(), constant(1).with_symbolic_limit())]);
    let small = ExprLimits {
        max_nodes: 1,
        max_depth: 1,
    };
    assert_eq!(compare(&source, domain, small), Err(sentinel));

    let key_only = BTreeMap::from([(complex.clone(), constant(1))]);
    let value_only = BTreeMap::from([(
        ExprId::fresh().unwrap(),
        constant(1).with_expression(complex.clone()),
    )]);
    for policy in [
        small,
        ExprLimits {
            max_depth: 1,
            ..limits(domain)
        },
    ] {
        let expected = Err(ExprError::Limit {
            nodes: complex.nodes(),
            depth: complex.depth(),
        });
        assert_eq!(compare(&key_only, domain, policy), expected);
        assert_eq!(compare(&value_only, domain, policy), expected);
    }
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(compare(&source, domain, small), Err(ExprError::Cancelled));
        // The old empty loop never validates or polls cancellation.
        assert_eq!(
            compare(&BTreeMap::new(), domain, small),
            Ok(BTreeMap::new())
        );
    });
}

#[test]
fn copy_import_preserves_nonempty_merges_and_atomic_failures() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let mut source = ByteArray::exact(&[0x11]);
    source.operands.insert(key.clone(), constant(2));
    source
        .operands
        .insert(ExprId::constant(U256::ZERO), constant(0));
    let source_before = source.clone();
    for occupied in [false, true] {
        let mut target = ByteArray::memory();
        if occupied {
            target.operands.insert(key.clone(), constant(1));
            // A logically preexisting neutral key follows the original merge
            // rule; the physical source clone alone must never create one.
            target
                .operands
                .insert(ExprId::constant(U256::ZERO), constant(0));
        }
        let mut expected = target.operands.clone();
        for (expression, value) in &source.operands {
            remember(
                &mut expected,
                expression.clone(),
                value,
                domain,
                limits(domain),
            )
            .unwrap();
        }
        for value in [constant(0), constant(0), constant(1)] {
            remember(
                &mut expected,
                expr(&value).unwrap(),
                &value,
                domain,
                limits(domain),
            )
            .unwrap();
        }
        target
            .copy_from(
                &constant(0),
                &source,
                &constant(0),
                &constant(1),
                64,
                domain,
            )
            .unwrap();
        assert_eq!(target.operands, expected);
        assert_eq!(
            target.operands.contains_key(&ExprId::constant(U256::ZERO)),
            occupied
        );
        assert_eq!(source, source_before);

        let mut rejected = source.clone();
        rejected
            .operands
            .insert(ExprId::fresh().unwrap(), constant(1).with_symbolic_limit());
        let rejected_before = rejected.clone();
        let mut target = if occupied {
            target
        } else {
            ByteArray::memory()
        };
        let before = target.clone();
        assert_eq!(
            target.copy_from(
                &constant(8),
                &rejected,
                &constant(0),
                &constant(1),
                64,
                domain
            ),
            Err(crate::world::bytes::RangeError::Expression(
                ExprError::Limit {
                    nodes: usize::MAX,
                    depth: usize::MAX,
                }
            ))
        );
        assert_eq!(target, before);
        assert_eq!(rejected, rejected_before);
    }
}
