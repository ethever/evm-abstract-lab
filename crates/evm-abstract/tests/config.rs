//! 配置准入的边界与实际执行一致，不能在引擎中重新猜测或采用默认域容量。

use evm_abstract::{
    Address, Fork, U256,
    analysis::{self, Config, ConfigError, ExecutionConfig, FrontierReason, Status, WorldAnalysis},
    bytecode::Program,
    domain::{AbstractValue, Profile},
    world::{Account, ByteArray, Entry, World},
};
use std::collections::BTreeSet;

#[test]
fn validation_reports_typed_errors_without_entering_the_engine() {
    assert!(matches!(
        Config {
            max_constants: 0,
            ..Config::default()
        }
        .validate(),
        Err(ConfigError::Constants)
    ));
    for config in [
        Config {
            max_states: 0,
            ..Config::default()
        },
        Config {
            max_transfers: 0,
            ..Config::default()
        },
    ] {
        assert!(matches!(config.validate(), Err(ConfigError::Budget)));
    }
}

#[test]
fn validated_boundary_values_keep_their_original_options() {
    for capacity in [1, 64, 65, 257, 1000, usize::MAX] {
        for depth in [0, 3, 8, 10, usize::MAX] {
            let validated = Config {
                max_constants: capacity,
                context_depth: depth,
                max_states: 1,
                max_transfers: 1,
                ..Config::default()
            }
            .validate()
            .unwrap();
            assert_eq!(validated.config().max_constants, capacity);
            assert_eq!(validated.config().context_depth, depth);
            assert_eq!(validated.config().max_states, 1);
            assert_eq!(validated.config().max_transfers, 1);
        }
    }
}

#[test]
fn default_context_depth_is_128() {
    assert_eq!(Config::default().context_depth, 128);
}

#[test]
fn default_constants_capacity_is_eight() {
    let validated = Config::default().validate().unwrap();
    assert_eq!(validated.config().max_constants, 8);
}

#[test]
fn single_program_execution_policy_controls_actual_memory_and_work() {
    // MSTORE beyond 64 KiB distinguishes an application policy from the smaller
    // library defaults used by the existing single-program adapter.
    let program = Program::from_hex("6001620100005200").unwrap();
    let legacy = analysis::analyze(program.clone(), Config::default()).unwrap();
    assert_eq!(legacy.status(), Status::Incomplete);
    assert!(
        legacy
            .execution()
            .frontiers()
            .iter()
            .any(|frontier| matches!(frontier.reason, FrontierReason::Memory))
    );
    let config = ExecutionConfig {
        max_memory_bytes: 128 * 1024,
        max_work: 100_000_000,
        ..ExecutionConfig::default()
    };
    let expanded = analysis::analyze_with_execution_config(
        program.clone(),
        config.clone(),
        Default::default(),
    )
    .unwrap();
    assert_eq!(expanded.status(), Status::Converged);
    assert_eq!(expanded.execution().config().max_memory_bytes, 128 * 1024);
    let bounded = analysis::analyze_with_execution_config(
        program,
        ExecutionConfig {
            max_work: 1,
            ..config
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(bounded.status(), Status::Incomplete);
    assert!(
        bounded
            .execution()
            .frontiers()
            .iter()
            .any(|frontier| matches!(frontier.reason, FrontierReason::Work))
    );
}

#[test]
fn admitted_domain_capacity_controls_the_actual_join() {
    let diamond = "600035600b576002600e565b60015b600a0100";
    for capacity in [1, 2] {
        let result = analysis::analyze(
            Program::from_hex(diamond).unwrap(),
            Config {
                max_constants: capacity,
                // 本测试检查域容量对汇合的影响，主动关闭上下文区分。
                context_depth: 0,
                ..Config::default()
            },
        )
        .unwrap();
        let merge = result
            .states()
            .iter()
            .find(|state| result.program().blocks()[state.key.basic_block_index].start_pc == 14)
            .unwrap();
        assert_eq!(result.config().max_constants, capacity);
        if capacity == 1 {
            assert!(merge.entry_stack[0].constants().is_none());
        } else {
            let values = merge.entry_stack[0].constants().unwrap();
            assert_eq!(values.len(), 2);
            assert!(values.contains(&U256::from(1)) && values.contains(&U256::from(2)));
        }
    }
}

fn analyze_unknown_calldata_clz(capacity: usize, max_work: usize) -> WorldAnalysis {
    let address = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "unbounded constants capacity regression");
    world
        .insert(address, Account::from_hex("5f351e00", Fork::Osaka).unwrap())
        .unwrap();
    analysis::analyze_world(
        world,
        Entry {
            address,
            environment: evm_abstract::world::EvmEnvironment {
                to: (address).into(),
                caller: (Address::ZERO).into(),
                value: AbstractValue::constant(U256::ZERO),
                calldata: ByteArray::unknown(),
                is_static: false,
                ..evm_abstract::world::EvmEnvironment::default()
            },
        },
        ExecutionConfig {
            analysis: Config {
                domain_profile: Profile::ConstantsOnly,
                max_constants: capacity,
                ..Config::default()
            },
            max_work,
            ..ExecutionConfig::default()
        },
    )
    .unwrap()
}

#[test]
fn clz_of_unknown_calldata_retains_all_257_candidates_when_capacity_allows() {
    let expected: BTreeSet<_> = (0_u64..=256).map(U256::from).collect();
    for capacity in [256, 257, 1000] {
        // CALLDATALOAD 的保守工作预扣随容量增长；容量与工作预算分别设置。
        let graph = analyze_unknown_calldata_clz(capacity, 100_000_000);
        assert_eq!(graph.config().analysis.max_constants, capacity);
        assert_eq!(graph.domain_spec().capacity(), capacity);
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert!(graph.diagnostics().is_empty());
        assert!(graph.frontiers().is_empty());
        assert_eq!(graph.states().len(), 1);
        let state = &graph.states()[0];
        assert_eq!(state.executed_pcs, [0, 1, 2, 3]);
        assert_eq!(state.exit_stack.len(), 1);
        if capacity == 256 {
            assert_eq!(
                state.exit_stack[0].numeric(),
                AbstractValue::top().numeric()
            );
            assert!(
                state.exit_stack[0].expression().is_some(),
                "capacity loss must retain CLZ's symbolic definition"
            );
        } else {
            assert_eq!(state.exit_stack[0].constants(), Some(&expected));
        }
    }
}

#[test]
fn maximum_capacity_can_execute_without_preallocating_the_limit() {
    let graph = analysis::analyze(
        Program::from_hex("00").unwrap(),
        Config {
            domain_profile: Profile::ConstantsOnly,
            max_constants: usize::MAX,
            max_states: 1,
            max_transfers: 1,
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(graph.config().max_constants, usize::MAX);
    assert_eq!(graph.execution().domain_spec().capacity(), usize::MAX);
    assert_eq!(graph.status(), Status::Converged);
    assert!(graph.frontiers().is_empty());
    assert_eq!(graph.states().len(), 1);
    assert_eq!(graph.states()[0].executed_pcs, [0]);
}

#[test]
fn large_constants_capacity_preserves_the_typed_shared_work_boundary() {
    // 足够完成入口投影，但不足以执行 CALLDATALOAD 的完整预扣。
    let max_work = 1_000_000;
    let graph = analyze_unknown_calldata_clz(1000, max_work);
    assert_eq!(graph.config().analysis.max_constants, 1000);
    assert_eq!(graph.status(), Status::Incomplete);
    assert!(graph.work() <= max_work);
    assert!(
        graph
            .frontiers()
            .iter()
            .any(|frontier| { frontier.reason == FrontierReason::Work && frontier.pc == Some(1) })
    );
}
