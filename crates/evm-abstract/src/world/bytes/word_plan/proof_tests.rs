//! Content proofs must survive the native read/identity/store boundary.

use super::*;
use alloy_primitives::Address;
use revm_bytecode::opcode;
use {
    crate::Fork, crate::analysis::ExecutionConfig, crate::analysis::OutcomeKind,
    crate::analysis::Status, crate::domain::provenance::Origin, crate::world::bytes::zero,
    crate::world::test_support::TestAccount, crate::world::test_support::TestEntry,
    crate::world::test_support::TestWorld,
};

#[test]
fn literal_loads_survive_runtime_identification_and_storage_joins() {
    for (load, origin) in [
        (opcode::MLOAD, Origin::Memory),
        (opcode::CALLDATALOAD, Origin::Calldata),
    ] {
        // Both CALLVALUE branches read zero at distinct offsets, then store
        // into slot0. A new unbound Fresh per read loses zero after their join.
        let code = format!("34600b575f{load:02x}5f556011565b6020{load:02x}5f555b00");
        let owner = Address::repeat_byte(0x31);
        let mut world = TestWorld::new(Fork::Osaka, "word-proof:runtime-store-join");
        world
            .insert(owner, TestAccount::from_hex(&code, Fork::Osaka).unwrap())
            .unwrap();
        let input = TestEntry::concrete(
            owner,
            Address::repeat_byte(0x32),
            TestValue::top(),
            TestBytes::empty(),
        );
        let graph = {
            let algebra =
                crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
            let policies = crate::analysis::EnginePolicies::new(
                crate::analysis::ContextInsensitive,
                crate::analysis::MergeResumeStatus,
            );
            let mut extension = crate::analysis::cooperation::NoExtension;
            let control = crate::analysis::control::Control::default();
            let engine = crate::analysis::AnalysisEngine::new(&algebra, &policies, &mut extension);
            let input = crate::analysis::AnalysisInput::new(world, input).unwrap();
            let mut engine = engine;
            engine.run(
                input,
                ExecutionConfig {
                    root_terminal_coverage: false,
                    ..ExecutionConfig::default()
                },
                &control,
            )
        }
        .unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert!(graph.states().iter().any(|state| {
            state.active().basic_block_index == 3
                && state.execution_evidence().unwrap().is_current()
        }));
        let returns: Vec<_> = graph
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .collect();
        assert!(!returns.is_empty());
        for outcome in returns {
            let value = outcome
                .store
                .read_checked(
                    owner,
                    &zero(),
                    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
                        .borrowed(),
                )
                .unwrap();
            assert_eq!(
                value.numeric(),
                zero::<crate::domain::NumericValue>().numeric()
            );
            assert!(
                value
                    .provenance()
                    .origins()
                    .sources()
                    .unwrap()
                    .contains(&origin)
            );
            assert!(!value.symbolic_limit_reached());
        }
    }
}

#[test]
fn prepared_and_complete_reads_keep_the_computed_literal_certificate() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut branches = [TestBytes::memory(), TestBytes::memory()];
    for (index, memory) in branches.iter_mut().enumerate() {
        memory
            .write_word(
                &TestValue::constant(U256::from(32)),
                &TestValue::constant(U256::from(index + 1))
                    .with_expression(ExprId::fresh().unwrap()),
                domain.borrowed(),
            )
            .unwrap();
    }
    let memory = branches[0]
        .join_guarded(
            &branches[1],
            domain.borrowed(),
            ExprId::fresh().unwrap(),
            limits(domain.borrowed()),
        )
        .unwrap();
    let frozen = memory.clone();
    for prepared in [true, false] {
        let mut budget = WorkBudget::new(100_000_000);
        let mut plan = memory
            .prepare_word_read(&zero(), domain.borrowed(), &mut budget)
            .unwrap()
            .unwrap();
        assert!(plan.preparation.is_some());
        if !prepared {
            // Force the complete algorithm, paying its full original fee in
            // addition to the already funded optional preparation.
            assert!(budget.charge(plan.fallback_work));
            plan.preparation = None;
        }
        let value = plan.read_word_budgeted(&mut budget).unwrap().unwrap();
        assert_eq!(
            value.numeric(),
            zero::<crate::domain::NumericValue>().numeric()
        );
        assert_eq!(
            value.expression().and_then(ExprId::as_constant),
            Some(U256::ZERO)
        );
        assert!(!budget.exhausted());
        assert_eq!(memory, frozen);
    }
}

#[test]
fn exact_nonliteral_word_sources_remain_independent() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut expressions = Vec::new();
    for _ in 0..2 {
        let source = ExprId::fresh().unwrap();
        let input = TestValue::constant(U256::from(7))
            .with_origin(Origin::Storage)
            .with_expression(source.clone());
        let mut memory = TestBytes::memory();
        memory
            .write_word(&zero(), &input, domain.borrowed())
            .unwrap();
        let mut budget = WorkBudget::new(100_000_000);
        let value = memory
            .prepare_word_read(&zero(), domain.borrowed(), &mut budget)
            .unwrap()
            .unwrap()
            .read_word_budgeted(&mut budget)
            .unwrap()
            .unwrap();
        assert_eq!(value.numeric(), input.numeric());
        assert_eq!(value.expression(), Some(&source));
        assert!(!value.symbolic_limit_reached());
        expressions.push(value.expression().unwrap().clone());
    }
    assert_ne!(expressions[0], expressions[1]);
}
