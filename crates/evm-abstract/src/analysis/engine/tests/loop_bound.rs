//! Symbolic input and initial state must close without enumerating the count.

#[cfg(test)]
use crate::analysis::testing::{Account, Entry, EvmEnvironment, TestValue, World};
use crate::domain::EvmNumericAlgebra as _;

use crate::{
    Fork,
    analysis::{ExecutionConfig, OutcomeKind, Status, machine::fixtures},
    domain::relational::RelationLimits,
};
use alloy_primitives::U256;

fn code_only(code: &str) -> (World, Entry) {
    let mut world = World::new(Fork::Osaka, "code-only loop regression");
    let account = Account {
        code: Account::from_hex(code, world.fork()).unwrap().code,
        ..Account::unknown()
    };
    let owner = fixtures::address(0x101);
    world.insert(owner, account).unwrap();
    let entry = Entry {
        address: owner,
        environment: EvmEnvironment {
            to: owner.into(),
            ..EvmEnvironment::default()
        },
    };
    (world, entry)
}

fn screen_policy() -> ExecutionConfig {
    let mut config = ExecutionConfig {
        max_work: 1_000_000_000_000,
        max_call_depth: 1025,
        max_memory_bytes: 67_108_864,
        ..ExecutionConfig::default()
    };
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    config.analysis.context_depth = 128;
    config_numeric.max_constants = 512;
    config_numeric.reduction_rounds = 16;
    config_numeric.max_facts = 4096;
    config.analysis.max_states = 100_000;
    config.analysis.max_transfers = 10_000_000;
    config_numeric.relations = RelationLimits {
        enabled: true,
        max_constraints: 2048,
        max_nodes: 16384,
        max_depth: 256,
    };
    config
}

fn same_word_witness(returned: &TestValue, stored: &TestValue, value: U256) -> bool {
    returned
        .symbolic_expression()
        .is_some_and(|expression| Some(expression) == stored.symbolic_expression())
        && returned.contains(value)
        && stored.contains(value)
}

#[test]
fn marginal_word_bounds_do_not_certify_a_joint_witness() {
    use crate::domain::{ProductAlgebra, symbolic::ExprId};
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let x = TestValue::top().with_expression(ExprId::fresh().unwrap());
    let next = domain.apply(revm_bytecode::opcode::ADD, &[x.clone(), fixtures::word(1)]);
    // Both margins contain 1, but x and x + 1 cannot jointly equal 1.
    assert!(x.contains(U256::ONE) && next.contains(U256::ONE));
    assert!(!same_word_witness(&x, &next, U256::ONE));
    assert!(same_word_witness(&x, &x, U256::ONE));
    assert!(same_word_witness(
        &fixtures::word(1),
        &fixtures::word(1),
        U256::ONE
    ));
}

#[test]
fn symbolic_countdown_closes_with_unknown_initial_state_and_joint_witnesses() {
    let input: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/no-smt-scale/code-only/loop.json"
    )))
    .unwrap();
    let code = input["accounts"][0]["code"].as_str().unwrap();
    assert_eq!((code.len() - 2) / 2, 33);
    let (world, entry) = code_only(code);
    let owner = entry.address;
    let config = screen_policy();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let analysis =
        crate::analysis::testing::execute(world, entry, config, config_numeric.spec()).unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(analysis.frontiers().is_empty());
    assert!(
        analysis.states().len() <= 24,
        "{} states",
        analysis.states().len()
    );
    assert!(
        analysis.transfers() <= 64,
        "{} transfers",
        analysis.transfers()
    );
    assert!(analysis.procedure_summaries().is_empty());
    for initial in [U256::ZERO, U256::from(5), U256::MAX - U256::ONE] {
        for count in [0, 1, 2, 9] {
            let expected = initial.wrapping_add(U256::from(count));
            assert!(
                analysis.outcomes().iter().any(|outcome| {
                    outcome.kind == OutcomeKind::Return
                        && outcome.data.len().contains(U256::from(32))
                        && same_word_witness(
                            &outcome.data.read_word(&fixtures::word(0), domain),
                            &outcome.store.read(owner, &fixtures::word(0), domain),
                            expected,
                        )
                }),
                "no joint return/store outcome for initial={initial}, count={count}"
            );
        }
    }
    // Unknown state is never silently filled with a zero slot observation.
    assert!(
        analysis
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .any(|outcome| outcome
                .store
                .read(owner, &fixtures::word(1), domain)
                .numeric()
                .is_top())
    );
}

#[test]
fn unknown_input_witnesses_share_one_return_and_storage_outcome() {
    let input: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/no-smt-scale/code-only/unknown-input.json"
    )))
    .unwrap();
    let (world, entry) = code_only(input["accounts"][0]["code"].as_str().unwrap());
    assert!(entry.environment.calldata.len().numeric().is_top());
    let owner = entry.address;
    let config = screen_policy();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let analysis =
        crate::analysis::testing::execute(world, entry, config, config_numeric.spec()).unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(analysis.frontiers().is_empty());
    // Empty input zero-pads; a short 0x2a input occupies the high byte; a full
    // word and arbitrarily longer calldata can contain any Word256 at offset 0.
    for word in [U256::ZERO, U256::from(42) << 248, U256::from(42), U256::MAX] {
        assert!(analysis.outcomes().iter().any(|outcome| {
            outcome.kind == OutcomeKind::Return
                && outcome.data.len().contains(U256::from(32))
                && outcome
                    .data
                    .read_word(&fixtures::word(0), domain)
                    .contains(word)
                && outcome
                    .store
                    .read(owner, &fixtures::word(0), domain)
                    .contains(word)
        }));
    }
}

#[test]
fn ordinary_jump_context_keeps_only_latest_sources_at_the_configured_depth() {
    let mut config = fixtures::config();
    let mut config_numeric = crate::analysis::testing::NumericLimits::machine();
    config.analysis.context_depth = 128;
    config.max_work = 1_000_000_000_000;
    let result = fixtures::run(
        "5f5b60010180601190106100015750602a5f5260205ff3",
        config,
        config_numeric.spec(),
    );
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    for state in result.states() {
        for frame in &state.key.frames {
            let mut sources = frame.jump_history.clone();
            sources.sort_unstable();
            sources.dedup();
            assert_eq!(sources.len(), frame.jump_history.len());
            assert!(frame.jump_history.len() <= 128);
        }
    }
}
