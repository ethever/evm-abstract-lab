//! 跳转历史深度决定精度；实际执行仍由共享资源预算限制。

use alloy_primitives::Address;
use evm_abstract::{
    Fork, U256,
    analysis::{
        self, Config, ExecutionConfig, FrontierReason, Limit, Status, WorldAnalysis, analyze_world,
    },
    bytecode::Program,
    domain::AbstractValue,
    ssa,
    world::{Account, ByteArray, Entry, World},
};

const SELF_LOOP: &str = "5b600056";

fn native(code: &str, config: ExecutionConfig) -> WorldAnalysis {
    let address = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "test:context-depth:fixed-world");
    world
        .insert(address, Account::from_hex(code, world.fork()).unwrap())
        .unwrap();
    analyze_world(
        world,
        Entry {
            address,
            environment: evm_abstract::world::EvmEnvironment {
                to: (address).into(),
                caller: (Address::ZERO).into(),
                value: AbstractValue::constant(U256::ZERO),
                calldata: ByteArray::empty(),
                is_static: false,
                ..evm_abstract::world::EvmEnvironment::default()
            },
        },
        config,
    )
    .unwrap()
}

#[test]
fn depth_ten_retains_ten_sources_and_then_closes_a_self_loop() {
    let graph = analysis::analyze(
        Program::from_hex(SELF_LOOP).unwrap(),
        Config {
            context_depth: 10,
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(graph.status(), Status::Converged);
    assert!(graph.frontiers().is_empty());
    let mut histories: Vec<_> = graph.states().iter().map(|s| &s.key.context).collect();
    histories.sort();
    assert_eq!(histories.len(), 11);
    for (length, history) in histories.into_iter().enumerate() {
        assert_eq!(*history, vec![0; length]);
    }
    let saturated = graph
        .states()
        .iter()
        .find(|state| state.key.context.len() == 10)
        .unwrap();
    assert!(
        graph
            .edges()
            .iter()
            .any(|edge| edge.from == saturated.id && edge.to == saturated.id)
    );
}

#[test]
fn large_requested_depth_does_not_preallocate_or_bypass_state_and_transfer_limits() {
    for (max_states, max_transfers, limit) in [(8, 64, Limit::States), (64, 8, Limit::Transfers)] {
        let graph = analysis::analyze(
            Program::from_hex(SELF_LOOP).unwrap(),
            Config {
                context_depth: usize::MAX,
                max_states,
                max_transfers,
                ..Config::default()
            },
        )
        .unwrap();
        assert_eq!(graph.config().context_depth, usize::MAX);
        assert_eq!(graph.status(), Status::Incomplete);
        assert!(graph.states().len() <= max_states);
        assert!(graph.transfers() <= max_transfers);
        assert!(
            graph
                .states()
                .iter()
                .any(|state| state.key.context.len() > 3)
        );
        assert!(
            graph
                .frontiers()
                .iter()
                .any(|frontier| frontier.limit == limit)
        );
        assert!(matches!(
            ssa::build(&graph),
            Err(ssa::SsaError::IncompleteAnalysis)
        ));
    }
}

#[test]
fn large_requested_depth_also_obeys_the_shared_work_limit() {
    let config = ExecutionConfig {
        analysis: Config {
            context_depth: usize::MAX,
            max_states: 1_000,
            max_transfers: 1_000,
            ..Config::default()
        },
        max_work: 5_000,
        use_summaries: false,
        ..ExecutionConfig::default()
    };
    let graph = native(SELF_LOOP, config.clone());
    assert_eq!(graph.status(), Status::Incomplete);
    assert!(graph.work() <= config.max_work);
    assert!(graph.states().len() < config.analysis.max_states);
    assert!(graph.transfers() < config.analysis.max_transfers);
    assert!(
        graph
            .states()
            .iter()
            .any(|state| state.active().jump_history.len() > 3)
    );
    assert!(
        graph
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Work)
    );
    assert!(ssa::build_world(&graph).is_err());
}

#[test]
fn finite_internal_calls_accept_depth_ten_and_maximum_usize() {
    let code = include_str!("../../../examples/internal-calls.hex");
    for depth in [10, usize::MAX] {
        let graph = analysis::analyze(
            Program::from_hex(code).unwrap(),
            Config {
                context_depth: depth,
                ..Config::default()
            },
        )
        .unwrap();
        assert_eq!(graph.config().context_depth, depth);
        assert_eq!(graph.status(), Status::Converged);
        assert!(graph.frontiers().is_empty());
        assert!(
            graph
                .states()
                .iter()
                .any(|state| state.key.context == [0, 14, 5, 14])
        );
        ssa::build(&graph).unwrap();
    }
}

#[test]
fn retained_history_contributes_to_work_even_when_the_graph_size_is_unchanged() {
    // 每个跳转只有一个后继，所以 k=0 与 k=10 的状态、transfer 数量相同。
    // 唯一额外载荷是实际保留的跳转历史，复制它也必须计入共享预算。
    let code = "600456005b600956005b600e56005b00";
    let run = |depth| {
        native(
            code,
            ExecutionConfig {
                analysis: Config {
                    context_depth: depth,
                    ..Config::default()
                },
                use_summaries: false,
                ..ExecutionConfig::default()
            },
        )
    };
    let insensitive = run(0);
    let sensitive = run(10);
    for graph in [&insensitive, &sensitive] {
        assert_eq!(graph.status(), Status::Converged);
        assert!(graph.frontiers().is_empty());
    }
    assert_eq!(insensitive.states().len(), sensitive.states().len());
    assert_eq!(insensitive.transfers(), sensitive.transfers());
    assert!(
        insensitive
            .states()
            .iter()
            .all(|state| state.active().jump_history.is_empty())
    );
    assert!(
        sensitive
            .states()
            .iter()
            .any(|state| state.active().jump_history == [0, 4, 9])
    );
    assert!(sensitive.work() > insensitive.work());
}
