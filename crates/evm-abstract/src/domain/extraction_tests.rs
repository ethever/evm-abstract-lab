//! Shared-domain adapters keep execution-only carrier fixtures in this crate.

use crate::{
    domain::{
        EvmNumericAlgebra, EvmValue, ProductAlgebra, SummaryConstruction, ValueAlgebra,
        WordObservation,
        summary::{canonical, join, top},
    },
    resource::{CloneError, WorkBudget},
};

mod symbolic;

#[test]
fn execution_reexports_share_the_foundation_types_and_payment_ledger() {
    let program: evm_bytecode::Program =
        crate::bytecode::Program::from_hex_with_fork("600160020100", crate::Fork::Osaka).unwrap();
    let old_program: crate::bytecode::Program = program.clone();
    let mut budget: evm_resource::WorkBudget = WorkBudget::new(16);
    assert!(program.eq_with_payment(&old_program, &mut budget).unwrap());
    assert_eq!(budget.used(), 16);
    let numeric: evm_domain::NumericValue = crate::domain::NumericValue::constant(crate::U256::ONE);
    let value: evm_domain::EvmValue<_> = EvmValue::from_numeric(numeric);
    assert_eq!(value.singleton(), Some(crate::U256::ONE));
}

#[test]
fn opaque_existing_algebras_decline_new_summary_hooks_by_default() {
    use crate::resource::clone_payment::tests::{
        Policy as Opaque, RECEIPT as OPAQUE_RECEIPT, Word as OpaqueWord,
    };
    let _scope = OPAQUE_RECEIPT.scope();
    let policy = Opaque(ProductAlgebra::default());
    let value = OpaqueWord::expensive(64);
    let borrowed = &policy;
    assert_eq!(
        EvmNumericAlgebra::summary_compact_work(&borrowed, &value),
        usize::MAX
    );
    assert_eq!(
        EvmNumericAlgebra::summary_restore_work(&borrowed, &value, &value),
        usize::MAX
    );
    assert_eq!(
        EvmNumericAlgebra::summary_observation_work(&borrowed, &WordObservation::top()),
        usize::MAX
    );
    assert_eq!(
        borrowed.summary_join_work(&value, &value, false),
        usize::MAX
    );
    assert_eq!(borrowed.summary_join_work(&value, &value, true), usize::MAX);
    assert_eq!(borrowed.summary_canonical_work(&value), usize::MAX);
    assert_eq!(
        borrowed.summary_construction_work(SummaryConstruction::Top),
        usize::MAX
    );
    assert_eq!(
        EvmNumericAlgebra::summary_equal_work(&borrowed, &value, &value),
        usize::MAX
    );
    assert_eq!(
        EvmNumericAlgebra::summary_is_top_work(&borrowed, &value),
        usize::MAX
    );
    let domain = ValueAlgebra::new(policy);
    let value = EvmValue::from_numeric(value);
    assert_eq!(
        join(
            &value,
            &value,
            domain.borrowed(),
            false,
            &mut WorkBudget::new(10_000)
        ),
        Err(CloneError::Work)
    );
    assert_eq!(
        canonical(&value, domain.borrowed(), &mut WorkBudget::new(10_000)),
        Err(CloneError::Work)
    );
    assert_eq!(
        top(domain.borrowed(), &mut WorkBudget::new(10_000)),
        Err(CloneError::Work)
    );
    assert_eq!(
        (OPAQUE_RECEIPT.get().clones, OPAQUE_RECEIPT.get().queries),
        (0, 0)
    );
}
