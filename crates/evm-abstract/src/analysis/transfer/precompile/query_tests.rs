//! Native extraction pays independent length and returned-byte discoveries.

use super::materialize;
use crate::{
    U256,
    analysis::{
        ExecutionConfig, FrontierReason,
        transfer::query::tests::{Policy, RECEIPT, Word, context},
    },
    domain::{EvmValue, ValueAlgebra, relational::QueryReason, symbolic::ByteStateKind},
    resource::WorkBudget,
    world::EvmByteArray,
};

#[test]
fn native_materialization_pays_each_opaque_discovery_at_its_own_boundary() {
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    // The byte case must reach a lazy view, rather than Exact's fast path.
    let view = EvmByteArray::<Word>::exact(&[0, 7])
        .slice_with(
            &EvmValue::constant(U256::ONE),
            &EvmValue::constant(U256::ONE),
            config.max_memory_bytes,
            algebra.borrowed(),
        )
        .unwrap();
    assert!(!matches!(view.state_root().kind(), ByteStateKind::Exact(_)));
    for (site, input, expected) in [
        ("length", EvmByteArray::<Word>::exact(&[7; 7]), vec![7; 7]),
        ("returned byte", view, vec![7]),
    ] {
        let _receipt = RECEIPT.scope();
        let mut complete = WorkBudget::new(1_000_000);
        assert_eq!(
            materialize(
                &input,
                &mut context(&config, &mut complete, algebra.borrowed())
            ),
            Ok(Some(expected)),
            "{site}"
        );
        assert_eq!(
            (RECEIPT.get().quotes, RECEIPT.get().queries),
            (1, 1),
            "{site}"
        );
        // Length discovery precedes all extraction; returned-byte discovery
        // follows the already paid view/read prefix. Keep both exact oracles.
        let prefix = if site == "length" {
            0
        } else {
            complete.used() - 61
        };
        let mut short = WorkBudget::new(prefix + 60);
        assert_eq!(
            materialize(
                &input,
                &mut context(&config, &mut short, algebra.borrowed())
            ),
            Err(FrontierReason::Work),
            "{site}: one short"
        );
        assert_eq!(short.used(), prefix, "{site}: one short");
        assert_eq!(
            (RECEIPT.get().quotes, RECEIPT.get().queries),
            (2, 1),
            "{site}"
        );
        RECEIPT.update(|receipt| receipt.fee = usize::MAX);
        let mut unlimited = WorkBudget::new(usize::MAX);
        assert_eq!(
            materialize(
                &input,
                &mut context(&config, &mut unlimited, algebra.borrowed())
            ),
            Err(FrontierReason::Work),
            "{site}: maximum quote"
        );
        assert_eq!(unlimited.used(), prefix, "{site}: maximum quote");
        assert_eq!(
            (RECEIPT.get().quotes, RECEIPT.get().queries),
            (3, 1),
            "{site}"
        );
        RECEIPT.update(|receipt| {
            receipt.fee = 61;
            receipt.cancel_on_query = true;
        });
        let token = analysis_control::Cancellation::new();
        analysis_control::with_cancellation(&token, || {
            let mut budget = WorkBudget::new(1_000_000);
            assert_eq!(
                materialize(
                    &input,
                    &mut context(&config, &mut budget, algebra.borrowed())
                ),
                Err(FrontierReason::Relations(QueryReason::Cancelled)),
                "{site}: query cancellation"
            );
            assert_eq!(budget.used(), prefix + 61, "{site}: query cancellation");
            assert_eq!(
                (RECEIPT.get().quotes, RECEIPT.get().queries),
                (4, 2),
                "{site}"
            );
        });
    }
}

#[test]
fn native_literal_length_can_decline_extraction_without_an_opaque_query() {
    let config = ExecutionConfig {
        max_memory_bytes: 6,
        ..ExecutionConfig::default()
    };
    let algebra = ValueAlgebra::new(Policy);
    let input = EvmByteArray::<Word>::exact(&[1; 8])
        .slice_with(
            &EvmValue::constant(U256::ZERO),
            &EvmValue::constant(U256::from(7)),
            config.max_memory_bytes,
            algebra.borrowed(),
        )
        .unwrap();
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.fee = usize::MAX;
        receipt.cancel_on_quote = true;
        receipt.cancel_on_query = true;
    });
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(None)
    );
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    let owner = analysis_control::Cancellation::new();
    let mut captured = analysis_control::with_cancellation(&owner, || WorkBudget::new(1_000_000));
    owner.cancel();
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut captured, algebra.borrowed())
        ),
        Err(FrontierReason::Relations(QueryReason::Cancelled))
    );
    assert_eq!(captured.used(), 0);
    let current = analysis_control::Cancellation::new();
    current.cancel();
    analysis_control::with_cancellation(&current, || {
        assert_eq!(
            materialize(
                &input,
                &mut context(&config, &mut budget, algebra.borrowed())
            ),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
    });
}
