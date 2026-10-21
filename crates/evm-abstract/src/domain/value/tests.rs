use super::{EvmValue, Origin, Symbol};
use crate::domain::{
    DomainSpec, EvmNumericAlgebra, FiniteConstantSet, FiniteSetAlgebra, NumericValue,
    ProductAlgebra, ValueAlgebra, symbolic::ExprId,
};
use alloy_primitives::U256;
use std::num::NonZeroUsize;

fn finite_algebra() -> ValueAlgebra<FiniteSetAlgebra> {
    ValueAlgebra::new(FiniteSetAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(256).unwrap(),
    )))
}

#[test]
fn numeric_layer_is_independent_of_sources_identities_and_expressions() {
    let numeric = NumericValue::unknown_address();
    let value = EvmValue::<NumericValue>::unknown_address()
        .with_origin(Origin::Storage)
        .with_symbol(Symbol::Caller, Some(1));
    assert_eq!(value.numeric(), &numeric);
    assert!(numeric.contains(U256::from(255)));
    assert!(!numeric.contains(U256::MAX));
    assert!(numeric.constants().is_none());
    assert!(value.identity().input().is_some());
    assert!(value.expression().is_some());
    let changed = value.clone().with_origin(Origin::Arithmetic);
    assert_eq!(changed.numeric(), value.numeric());
    assert_eq!(changed.identity(), value.identity());
    assert_eq!(changed.expression(), value.expression());
    assert_ne!(changed.provenance(), value.provenance());
}

#[test]
fn native_serialization_keeps_numeric_components_and_metadata_separate() {
    let value = EvmValue::<NumericValue>::unknown_address().with_symbol(Symbol::Caller, Some(1));
    let json = serde_json::to_value(&value).unwrap();
    assert!(json.is_object());
    for key in ["known_bits", "interval"] {
        assert!(json["numeric"].get(key).is_some());
    }
    assert_eq!(json["identity"]["input"]["name"], "Caller");
    assert!(json["provenance"].get("symbol").is_none());
    assert!(json["provenance"].get("identity").is_none());
    assert_eq!(
        json["numeric"],
        serde_json::to_value(value.numeric()).unwrap()
    );
    assert!(json["numeric"].get("constants").is_none());
    assert!(json.get("expression").is_some());
    let constant =
        serde_json::to_value(EvmValue::<NumericValue>::constant(U256::from(42))).unwrap();
    assert_eq!(
        constant["numeric"]["constants"],
        serde_json::json!(["0x2a"])
    );
    let numeric_top = serde_json::to_value(NumericValue::top()).unwrap();
    assert!(numeric_top.is_object());
    assert_eq!(numeric_top, serde_json::json!({}));
    let top = serde_json::to_value(EvmValue::<NumericValue>::top()).unwrap();
    assert!(top.is_object());
    assert_eq!(top["numeric"], numeric_top);
    assert!(top.get("provenance").is_some());
}

#[test]
fn numeric_projection_keeps_sources_roles_input_identity_and_symbolic_expression() {
    let value =
        EvmValue::<FiniteConstantSet>::unknown_address().with_symbol(Symbol::Caller, Some(1));
    let projected = finite_algebra().project(&value);
    assert!(projected.numeric().is_top());
    assert_eq!(projected.provenance(), value.provenance());
    assert_eq!(projected.identity(), value.identity());
    assert_eq!(projected.expression(), value.expression());
}

#[test]
fn joins_and_widening_retain_only_must_expression_equalities() {
    fn check<D: EvmNumericAlgebra>(domain: ValueAlgebra<D>) {
        let first = EvmValue::<D::Element>::top().with_expression(ExprId::input(1, Symbol::Caller));
        let same = first.clone();
        let other = EvmValue::<D::Element>::top().with_expression(ExprId::input(1, Symbol::Origin));
        assert_eq!(domain.join(&first, &same).expression(), first.expression());
        assert!(domain.join(&first, &other).expression().is_none());
        assert_eq!(domain.widen(&first, &same).expression(), first.expression());
        assert!(domain.widen(&first, &other).expression().is_none());
    }
    check(ValueAlgebra::new(ProductAlgebra::default()));
    check(finite_algebra());
}

#[test]
fn numeric_refinement_keeps_the_original_value_layers() {
    let value = EvmValue::<NumericValue>::unknown_address().with_symbol(Symbol::Caller, Some(1));
    let refined = value
        .clone()
        .with_numeric(NumericValue::constant(U256::from(5)));
    assert_eq!(refined.singleton(), Some(U256::from(5)));
    assert_eq!(refined.identity(), value.identity());
    assert_eq!(refined.expression(), value.expression());
    assert_eq!(refined.provenance(), value.provenance());
    assert_eq!(
        refined.symbolic_expression(),
        Some(ExprId::constant(U256::from(5)))
    );
}

#[test]
fn pure_operations_retain_symbolic_inputs_without_changing_numeric_results() {
    fn check<D: EvmNumericAlgebra>(domain: ValueAlgebra<D>) {
        let input = EvmValue::<D::Element>::top().with_symbol(Symbol::CallValue, Some(7));
        let output = domain.apply(
            revm_bytecode::opcode::ADD,
            &[
                input.clone(),
                EvmValue::<D::Element>::constant(U256::from(1)),
            ],
        );
        assert!(output.singleton().is_none());
        assert!(output.expression().is_some());
        assert_eq!(
            output.expression(),
            Some(
                &ExprId::operation(
                    revm_bytecode::opcode::ADD,
                    &[
                        ExprId::input(7, Symbol::CallValue),
                        ExprId::constant(U256::from(1))
                    ],
                    domain.spec().relations().expressions()
                )
                .unwrap()
            )
        );
    }
    check(ValueAlgebra::new(ProductAlgebra::default()));
    check(finite_algebra());
}

#[test]
fn expression_limit_keeps_sound_numeric_facts_and_a_typed_failure() {
    let input = EvmValue::<NumericValue>::unknown_address().with_symbol(Symbol::Caller, Some(1));
    let policy = crate::domain::relational::RelationLimits {
        max_nodes: 1,
        ..Default::default()
    };
    let domain = ValueAlgebra::new(ProductAlgebra::from_spec(
        DomainSpec::new(
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
        .with_relations(policy),
    ));
    let result = domain.apply_detailed(
        revm_bytecode::opcode::ADD,
        &[input, EvmValue::<NumericValue>::constant(U256::from(1))],
    );
    assert!(result.value.expression().is_none());
    assert!(result.value.contains(U256::from(1)));
    assert!(matches!(
        result.symbolic_error,
        Some(crate::domain::symbolic::ExprError::Limit { .. })
    ));
}

#[test]
fn canonical_symbolic_constants_feed_back_into_numeric_components() {
    let domain = finite_algebra();
    let input = EvmValue::<FiniteConstantSet>::top().with_symbol(Symbol::CallValue, Some(7));
    let derived = domain.apply(
        revm_bytecode::opcode::ADD,
        &[
            input,
            EvmValue::<FiniteConstantSet>::constant(U256::from(1)),
        ],
    );
    assert!(derived.singleton().is_none());
    assert!(derived.identity().input().is_none());
    let result = domain.apply(revm_bytecode::opcode::XOR, &[derived.clone(), derived]);
    assert_eq!(result.singleton(), Some(U256::ZERO));
    assert_eq!(result.numeric(), &FiniteConstantSet::constant(U256::ZERO));
    assert_eq!(
        result.symbolic_expression(),
        Some(ExprId::constant(U256::ZERO))
    );
}

#[test]
fn disabling_relations_forgets_expression_but_preserves_input_identity_and_sources() {
    let input = EvmValue::<NumericValue>::unknown_address().with_symbol(Symbol::Caller, Some(1));
    let limits = crate::domain::relational::RelationLimits {
        enabled: false,
        ..Default::default()
    };
    let domain = ValueAlgebra::new(ProductAlgebra::from_spec(
        DomainSpec::new(
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
        .with_relations(limits),
    ));
    let projected = domain.project(&input);
    assert!(projected.expression().is_none());
    assert_eq!(projected.identity(), input.identity());
    assert_eq!(projected.provenance(), input.provenance());
    assert_eq!(projected.numeric(), input.numeric());
}

#[test]
fn symbolic_limit_survives_numeric_only_callers_and_clears_on_exact_results() {
    let limits = crate::domain::relational::RelationLimits {
        max_nodes: 1,
        ..Default::default()
    };
    let domain = ValueAlgebra::new(ProductAlgebra::from_spec(
        DomainSpec::new(
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
        .with_relations(limits),
    ));
    let input = EvmValue::<NumericValue>::top().with_symbol(Symbol::Caller, Some(1));
    let lost = domain.apply(
        revm_bytecode::opcode::ADD,
        &[input, EvmValue::<NumericValue>::constant(U256::from(1))],
    );
    assert!(lost.symbolic_limit_reached());
    assert!(lost.expression().is_none());
    assert_eq!(serde_json::to_value(&lost).unwrap()["symbolic_limit"], true);
    let inherited = ValueAlgebra::new(ProductAlgebra::default()).apply(
        revm_bytecode::opcode::ADD,
        &[lost.clone(), EvmValue::<NumericValue>::top()],
    );
    assert!(inherited.symbolic_limit_reached());
    assert!(
        ValueAlgebra::new(ProductAlgebra::default())
            .join(&lost, &EvmValue::<NumericValue>::top())
            .symbolic_limit_reached()
    );
    let exact = ValueAlgebra::new(ProductAlgebra::default()).apply(
        revm_bytecode::opcode::MUL,
        &[lost.clone(), EvmValue::<NumericValue>::constant(U256::ZERO)],
    );
    assert_eq!(exact.singleton(), Some(U256::ZERO));
    assert!(!exact.symbolic_limit_reached());
    let disabled = ValueAlgebra::new(ProductAlgebra::from_spec(domain.spec().with_relations(
        crate::domain::relational::RelationLimits {
            enabled: false,
            ..Default::default()
        },
    )));
    assert!(!disabled.project(&lost).symbolic_limit_reached());
}

#[test]
fn transient_copy_identity_is_shared_by_native_carriers_and_relation_switches() {
    fn check<D: EvmNumericAlgebra>(domain: ValueAlgebra<D>) {
        let input = EvmValue::<D::Element>::top().with_identity(3, 4);
        assert_eq!(
            domain
                .apply(revm_bytecode::opcode::XOR, &[input.clone(), input.clone()])
                .singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(
            domain
                .apply(revm_bytecode::opcode::EQ, &[input.clone(), input.clone()])
                .singleton(),
            Some(U256::from(1))
        );
    }
    for enabled in [true, false] {
        let spec =
            DomainSpec::default().with_relations(crate::domain::relational::RelationLimits {
                enabled,
                ..Default::default()
            });
        check(ValueAlgebra::new(ProductAlgebra::from_spec(spec)));
        check(ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec)));
    }
}
