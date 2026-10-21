//! 配置准入的边界与实际执行一致，不能在引擎中重新猜测或采用默认域容量。
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisInput, EvmAnalysis};
use evm_abstract::domain::FiniteConstantSet;

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        self, Config, ConfigError, EvmWorldAnalysis, ExecutionConfig, FrontierReason, Status,
    },
    bytecode::Program,
    domain::EvmValue,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use std::collections::BTreeSet;

#[test]
fn validation_reports_typed_errors_without_entering_the_engine() {
    assert!(std::num::NonZeroUsize::new(0).is_none());
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
                context_depth: depth,
                max_states: 1,
                max_transfers: 1,
                ..Config::default()
            }
            .validate()
            .unwrap();
            assert_eq!(native::spec(capacity, 4, 256).capacity(), capacity);
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
    assert_eq!(native::product().spec().capacity(), 8);
}

#[test]
fn single_program_execution_policy_controls_materialization_and_actual_work() {
    // MSTORE beyond 64 KiB is a lazy logical write under either host cap.
    let program = Program::from_hex("6001620100005200").unwrap();
    let legacy = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(program.clone(), Default::default()).unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    assert_eq!(legacy.status(), Status::Converged);
    assert!(legacy.execution().frontiers().is_empty());
    assert!(
        legacy
            .execution()
            .outcomes()
            .iter()
            .any(|o| o.kind == analysis::OutcomeKind::Return)
    );
    assert!(
        legacy
            .execution()
            .outcomes()
            .iter()
            .any(|o| o.kind == analysis::OutcomeKind::Failure)
    );
    let config = ExecutionConfig {
        max_memory_bytes: 128 * 1024,
        max_work: 100_000_000,
        ..ExecutionConfig::default()
    };
    let expanded = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(program.clone(), Default::default()).unwrap(),
        config.clone(),
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    assert_eq!(expanded.status(), Status::Converged);
    assert_eq!(expanded.execution().config().max_memory_bytes, 128 * 1024);
    let bounded = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(program, Default::default()).unwrap(),
        ExecutionConfig {
            max_work: 1,
            ..config
        },
        &Control::default(),
    )
    .unwrap();
    assert_eq!(bounded.status(), Status::Incomplete);
    assert!(
        bounded
            .frontiers()
            .iter()
            .any(|frontier| matches!(frontier.reason, FrontierReason::Work))
    );
    assert!(matches!(
        EvmAnalysis::from_execution(bounded, &mut native::postprocessing()),
        Err(evm_abstract::analysis::ProjectionError::MissingProgram)
    ));
}

#[test]
fn admitted_domain_capacity_controls_the_actual_join() {
    let diamond = "600035600b576002600e565b60015b600a0100";
    for capacity in [1, 2] {
        let result = native::run(
            &native::product_with(native::spec(capacity, 4, 256)),
            AnalysisInput::<NumericValue>::from_program(
                Program::from_hex(diamond).unwrap(),
                Default::default(),
            )
            .unwrap(),
            ExecutionConfig {
                analysis: Config {
                    // 本测试检查域容量对汇合的影响，主动关闭上下文区分。
                    context_depth: 0,
                    ..Config::default()
                },
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
        .unwrap();
        let merge = result
            .states()
            .iter()
            .find(|state| result.program().blocks()[state.key.basic_block_index].start_pc == 14)
            .unwrap();
        assert_eq!(result.execution().domain_spec().capacity(), capacity);
        if capacity == 1 {
            assert!(merge.entry_stack[0].constants().is_none());
        } else {
            let values = merge.entry_stack[0].constants().unwrap();
            assert_eq!(values.len(), 2);
            assert!(values.contains(&U256::from(1)) && values.contains(&U256::from(2)));
        }
    }
}

fn unknown_calldata_clz(capacity: usize, max_work: usize) -> EvmWorldAnalysis<FiniteConstantSet> {
    let address = Address::repeat_byte(0x11);
    let mut world =
        EvmWorld::<FiniteConstantSet>::new(Fork::Osaka, "unbounded constants capacity regression");
    world
        .insert(
            address,
            EvmAccount::<FiniteConstantSet>::from_hex("5f351e00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    native::run(
        &native::finite_with(native::spec(capacity, 4, 256)),
        AnalysisInput::new(
            world,
            EvmEntry {
                address,
                environment: evm_abstract::world::EvmEnvironmentValues {
                    to: (address).into(),
                    caller: (Address::ZERO).into(),
                    value: EvmValue::<FiniteConstantSet>::constant(U256::ZERO),
                    calldata: EvmByteArray::<FiniteConstantSet>::unknown(),
                    is_static: false,
                    ..evm_abstract::world::EvmEnvironmentValues::<FiniteConstantSet>::default()
                },
            },
        )
        .unwrap(),
        ExecutionConfig {
            analysis: Config::default(),
            max_work,
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .unwrap()
}

#[test]
fn clz_of_unknown_calldata_retains_all_257_candidates_when_capacity_allows() {
    let expected: BTreeSet<_> = (0_u64..=256).map(U256::from).collect();
    for capacity in [256, 257, 1000] {
        // CALLDATALOAD 的保守工作预扣随容量增长；容量与工作预算分别设置。
        let graph = unknown_calldata_clz(capacity, 100_000_000);
        assert_eq!(graph.domain_spec().capacity(), capacity);
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
                EvmValue::<FiniteConstantSet>::top().numeric()
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
    let graph = native::run(
        &native::finite_with(native::spec(usize::MAX, 4, 256)),
        AnalysisInput::<FiniteConstantSet>::from_program(
            Program::from_hex("00").unwrap(),
            Default::default(),
        )
        .unwrap(),
        ExecutionConfig {
            analysis: Config {
                max_states: 1,
                max_transfers: 1,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    assert_eq!(graph.execution().domain_spec().capacity(), usize::MAX);
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
    let graph = unknown_calldata_clz(1000, max_work);
    assert_eq!(graph.domain_spec().capacity(), 1000);
    assert_eq!(graph.status(), Status::Incomplete);
    assert!(graph.work() <= max_work);
    assert!(
        graph
            .frontiers()
            .iter()
            .any(|frontier| { frontier.reason == FrontierReason::Work && frontier.pc == Some(1) })
    );
}
