//! 跳转历史深度决定精度；实际执行仍由共享资源预算限制。
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisInput, EvmAnalysis};

#[path = "support/native.rs"]
mod native;

use alloy_primitives::Address;
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork, U256,
    analysis::{
        self, Config, EdgeKind, EvmWorldAnalysis, ExecutionConfig, FrontierReason, Limit, Status,
    },
    bytecode::Program,
    domain::EvmValue,
    ssa,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use revm_bytecode::opcode;
use std::collections::BTreeSet;

const SELF_LOOP: &str = "5b600056";

// Work backwards through the complete trace, independently of transfer's
// remove/append/truncate implementation.
fn latest_distinct(sources: &[usize], depth: usize) -> Vec<usize> {
    let mut seen = BTreeSet::new();
    let mut newest: Vec<_> = sources
        .iter()
        .rev()
        .copied()
        .filter(|source| seen.insert(*source))
        .take(depth)
        .collect();
    newest.reverse();
    newest
}

fn jump_chain(source_count: u8, backedge: Option<u8>) -> Program {
    assert!(source_count > 0);
    assert!(backedge.is_none_or(|target| target < source_count));
    let count = usize::from(source_count);
    let stop_pc = count.checked_mul(5).unwrap();
    u16::try_from(stop_pc).unwrap();
    let mut bytes = Vec::with_capacity(stop_pc.checked_add(2).unwrap());
    for index in 0..count {
        let next = index.checked_add(1).unwrap();
        let target = if next == count {
            backedge.map_or(count, usize::from)
        } else {
            next
        };
        let target = u16::try_from(target.checked_mul(5).unwrap())
            .unwrap()
            .to_be_bytes();
        bytes.extend([
            opcode::JUMPDEST,
            opcode::PUSH2,
            target[0],
            target[1],
            opcode::JUMP,
        ]);
    }
    bytes.extend([opcode::JUMPDEST, opcode::STOP]);
    let program = Program::decode(&bytes).unwrap();
    assert_eq!(program.blocks().len(), count.checked_add(1).unwrap());
    assert_eq!(program.blocks()[count].start_pc, stop_pc);
    assert_eq!(
        program.blocks()[count]
            .instructions
            .iter()
            .map(|i| i.opcode)
            .collect::<Vec<_>>(),
        [opcode::JUMPDEST, opcode::STOP]
    );
    for (index, block) in program.blocks()[..count].iter().enumerate() {
        assert_eq!(block.start_pc, index.checked_mul(5).unwrap());
        assert_eq!(
            block
                .instructions
                .iter()
                .map(|i| i.opcode)
                .collect::<Vec<_>>(),
            [opcode::JUMPDEST, opcode::PUSH2, opcode::JUMP]
        );
        let next = index.checked_add(1).unwrap();
        let target = if next == count {
            backedge.map_or(count, usize::from)
        } else {
            next
        };
        let target_pc = program.blocks()[target].start_pc;
        assert_eq!(block.instructions[1].immediate, Some(U256::from(target_pc)));
        assert_eq!(program.jumpdest_blocks().get(&target_pc), Some(&target));
    }
    program
}

fn state_after_sources<'a>(
    graph: &'a analysis::EvmAnalysis<NumericValue>,
    pc: usize,
    sources: &[usize],
    depth: usize,
) -> &'a analysis::EvmState<NumericValue> {
    let context = latest_distinct(sources, depth);
    graph
        .states()
        .iter()
        .find(|state| {
            graph.program().blocks()[state.key.basic_block_index].start_pc == pc
                && state.key.context == context
        })
        .unwrap()
}

fn assert_distinct_contexts(graph: &analysis::EvmAnalysis<NumericValue>, depth: usize) {
    for state in graph.states() {
        assert!(state.key.context.len() <= depth);
        assert_eq!(
            state.key.context,
            latest_distinct(&state.key.context, depth)
        );
        for source in &state.key.context {
            let block = graph
                .program()
                .blocks()
                .iter()
                .find(|b| b.start_pc == *source)
                .unwrap();
            assert!(matches!(
                block.instructions.last().unwrap().opcode,
                opcode::JUMP | opcode::JUMPI
            ));
        }
    }
}

fn assert_jump_trace(
    graph: &analysis::EvmAnalysis<NumericValue>,
    pcs: &[usize],
    depth: usize,
) -> Vec<usize> {
    let mut sources = Vec::new();
    let mut states = Vec::new();
    for pc in pcs {
        let state = state_after_sources(graph, *pc, &sources, depth);
        let block = &graph.program().blocks()[state.key.basic_block_index];
        assert_eq!(
            state.executed_pcs,
            block.instructions.iter().map(|i| i.pc).collect::<Vec<_>>()
        );
        if let Some(previous) = states.last() {
            assert!(graph.edges().iter().any(|edge| {
                edge.from == *previous
                    && edge.to == state.id
                    && edge.kind
                        == evm_abstract::analysis::MachineEdgeKind::Intraprocedural(EdgeKind::Jump)
            }));
        }
        states.push(state.id);
        sources.push(*pc);
    }
    states
}

fn native(code: &str, config: ExecutionConfig) -> EvmWorldAnalysis<NumericValue> {
    let address = Address::repeat_byte(0x11);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "test:context-depth:fixed-world");
    world
        .insert(
            address,
            EvmAccount::<NumericValue>::from_hex(code, world.fork()).unwrap(),
        )
        .unwrap();
    native::run(
        &native::product(),
        AnalysisInput::new(
            world,
            EvmEntry {
                address,
                environment: evm_abstract::world::EvmEnvironmentValues {
                    to: (address).into(),
                    caller: (Address::ZERO).into(),
                    value: EvmValue::<NumericValue>::constant(U256::ZERO),
                    calldata: EvmByteArray::<NumericValue>::empty(),
                    is_static: false,
                    ..evm_abstract::world::EvmEnvironmentValues::<NumericValue>::default()
                },
            },
        )
        .unwrap(),
        config,
        &Control::default(),
    )
    .unwrap()
}

#[test]
fn depth_ten_retains_distinct_sources_and_closes_a_self_loop() {
    let graph = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex(SELF_LOOP).unwrap(),
            Default::default(),
        )
        .unwrap(),
        ExecutionConfig {
            analysis: Config {
                context_depth: 10,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    assert_eq!(graph.status(), Status::Converged);
    assert!(graph.frontiers().is_empty());
    let mut histories: Vec<_> = graph
        .states()
        .iter()
        .map(|s| s.key.context.clone())
        .collect();
    histories.sort();
    assert_eq!(histories, [vec![], vec![0]]);
    let saturated = graph
        .states()
        .iter()
        .find(|state| state.key.context == [0])
        .unwrap();
    assert!(graph.edges().iter().any(|edge| edge.from == saturated.id
        && edge.to == saturated.id
        && edge.kind == evm_abstract::analysis::MachineEdgeKind::Intraprocedural(EdgeKind::Jump)));
    assert_distinct_contexts(&graph, 10);
    assert_jump_trace(&graph, &[0, 0, 0], 10);
    ssa::build(&graph).unwrap().verify(&graph).unwrap();

    for depth in [0, 10, usize::MAX] {
        for backedge in [None, Some(4)] {
            let program = jump_chain(12, backedge);
            let source_pcs: Vec<_> = program.blocks()[..12]
                .iter()
                .map(|block| block.start_pc)
                .collect();
            assert_eq!(
                source_pcs.iter().copied().collect::<BTreeSet<_>>().len(),
                12
            );
            let mut path = source_pcs.clone();
            if let Some(target) = backedge {
                let cycle = &source_pcs[usize::from(target)..];
                path.extend_from_slice(cycle);
                path.extend_from_slice(cycle);
                path.push(cycle[0]);
            } else {
                path.push(program.blocks()[12].start_pc);
            }
            let graph = native::run(
                &native::product(),
                AnalysisInput::<NumericValue>::from_program(program, Default::default()).unwrap(),
                ExecutionConfig {
                    analysis: Config {
                        context_depth: depth,
                        ..Config::default()
                    },
                    ..ExecutionConfig::default()
                },
                &Control::default(),
            )
            .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
            .unwrap();
            assert_eq!(graph.config().context_depth, depth);
            assert_eq!(graph.status(), Status::Converged);
            assert!(graph.frontiers().is_empty());
            assert_distinct_contexts(&graph, depth);
            let trace = assert_jump_trace(&graph, &path, depth);
            assert_eq!(
                trace.iter().copied().collect::<BTreeSet<_>>().len(),
                graph.states().len()
            );
            assert!(
                graph
                    .states()
                    .iter()
                    .all(|state| state.entry_stack.is_empty() && state.exit_stack.is_empty())
            );
            let saturated = state_after_sources(&graph, path[12], &source_pcs, depth);
            assert_eq!(saturated.key.context.len(), depth.min(source_pcs.len()));
            if backedge.is_some() {
                let mut revisited = source_pcs.clone();
                revisited.push(source_pcs[4]);
                let updated = state_after_sources(&graph, source_pcs[5], &revisited, depth);
                if depth > 0 {
                    assert_eq!(updated.key.context.last(), Some(&source_pcs[4]));
                    assert_ne!(updated.key.context, saturated.key.context);
                }
            }
            ssa::build(&graph).unwrap().verify(&graph).unwrap();
        }
    }
}

#[test]
fn large_requested_depth_obeys_state_and_transfer_limits() {
    let program = jump_chain(12, None);
    let pcs: Vec<_> = program
        .blocks()
        .iter()
        .map(|block| block.start_pc)
        .collect();
    for (max_states, max_transfers, limit) in [(8, 64, Limit::States), (64, 8, Limit::Transfers)] {
        let graph = native::run(
            &native::product(),
            AnalysisInput::<NumericValue>::from_program(program.clone(), Default::default())
                .unwrap(),
            ExecutionConfig {
                analysis: Config {
                    context_depth: usize::MAX,
                    max_states,
                    max_transfers,
                },
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
        .unwrap();
        assert_eq!(graph.config().context_depth, usize::MAX);
        assert_eq!(graph.status(), Status::Incomplete);
        assert!(graph.states().len() <= max_states);
        assert!(graph.transfers() <= max_transfers);
        assert_eq!(graph.transfers(), 8);
        assert_distinct_contexts(&graph, usize::MAX);
        assert_jump_trace(&graph, &pcs[..8], usize::MAX);
        let source = state_after_sources(&graph, pcs[7], &pcs[..7], usize::MAX);
        assert_eq!(source.key.context.len(), 7);
        assert_eq!(graph.frontiers().len(), 1);
        let frontier = &graph.frontiers()[0];
        assert_eq!(frontier.limit, limit);
        assert_eq!(frontier.reason, FrontierReason::Budget(limit));
        assert_eq!(frontier.target.basic_block_index, 8);
        assert_eq!(frontier.target.stack_height, 0);
        assert_eq!(frontier.target.context, pcs[..8]);

        let native = graph.execution();
        assert_eq!(native.transfers(), graph.transfers());
        assert_eq!(native.states().len(), graph.states().len());
        assert_eq!(native.frontiers().len(), 1);
        let native_source = &native.states()[graph.execution_state_id(source.id).unwrap()];
        assert_eq!(native_source.entry.key(), native_source.key);
        let source_exit = native_source.exit.as_ref().unwrap();
        assert_eq!(native_source.exit_stack, source_exit.active().stack);
        let native_frontier = &native.frontiers()[0];
        assert_eq!(native_frontier.reason, FrontierReason::Budget(limit));
        assert_eq!(native_frontier.pc, None);
        let target = native_frontier.target.as_ref().unwrap();
        assert_eq!(target.frames.len(), 1);
        assert_eq!(target.frames[0].basic_block_index, 8);
        assert_eq!(target.frames[0].jump_history, pcs[..8]);
        let incoming = match limit {
            Limit::States => {
                assert_eq!(graph.states().len(), max_states);
                assert!(graph.transfers() < max_transfers);
                assert_eq!(frontier.from, Some(source.id));
                assert_eq!(native_frontier.from, Some(native_source.id));
                native_frontier.payload.as_deref().unwrap()
            }
            Limit::Transfers => {
                assert_eq!(graph.transfers(), max_transfers);
                assert_eq!(graph.states().len(), 9);
                assert!(graph.states().len() < max_states);
                assert_eq!(frontier.from, None);
                assert_eq!(native_frontier.from, None);
                assert!(native_frontier.payload.is_none());
                let pending = state_after_sources(&graph, pcs[8], &pcs[..8], usize::MAX);
                assert!(pending.executed_pcs.is_empty());
                assert!(graph.edges().iter().any(|edge| edge.from == source.id
                    && edge.to == pending.id
                    && edge.kind
                        == evm_abstract::analysis::MachineEdgeKind::Intraprocedural(
                            EdgeKind::Jump
                        )));
                let pending = &native.states()[graph.execution_state_id(pending.id).unwrap()];
                assert!(pending.exit.is_none());
                &pending.entry
            }
            _ => unreachable!(),
        };
        assert_eq!(incoming.key(), *target);
        let mut expected_frame = source_exit.active().clone();
        expected_frame.key.basic_block_index = 8;
        assert_eq!(incoming.active(), &expected_frame);
        assert_eq!(incoming.store, source_exit.store);
        assert_eq!(incoming.relations, source_exit.relations);
        assert!(matches!(
            ssa::build(&graph),
            Err(ssa::SsaError::IncompleteAnalysis)
        ));
        assert!(matches!(
            ssa::build_world(native),
            Err(ssa::SsaError::IncompleteAnalysis)
        ));
    }
}

#[test]
fn large_requested_depth_also_obeys_the_shared_work_limit() {
    use analysis::progress::{Event, Observer};

    let config = ExecutionConfig {
        analysis: Config {
            context_depth: usize::MAX,
            max_states: 1_000,
            max_transfers: 1_000,
        },
        max_work: 5_000,
        use_summaries: false,
        ..ExecutionConfig::default()
    };
    // Retain the original 5k resource control. Paid entry/input preparation can
    // refuse before any instruction, so it cannot witness jump-history work.
    let admission = native(SELF_LOOP, config.clone());
    assert_eq!(admission.transfers(), 0);
    assert!(admission.edges().is_empty());
    assert!(
        admission
            .states()
            .iter()
            .all(|state| state.executed_pcs.is_empty())
    );

    // Independently measure one complete transfer with a transfer cap, keeping
    // the same original inputs, default root coverage and precision policies.
    // The first execution checkpoint follows paid entry/input admission; the
    // report also includes the first jump's successor and pending-stop work.
    let (sender, events) = std::sync::mpsc::sync_channel(32);
    let observer = Observer::new(sender);
    let mut one_transfer_config = config.clone();
    one_transfer_config.max_work = ExecutionConfig::default().max_work;
    one_transfer_config.analysis.max_transfers = 1;
    let one_transfer = native::run(
        &native::product(),
        AnalysisInput::new(admission.world().clone(), admission.entry().clone()).unwrap(),
        one_transfer_config,
        &Control::new(observer.clone()),
    )
    .unwrap();
    assert_eq!(one_transfer.transfers(), 1);
    assert_eq!(one_transfer.frontiers().len(), 1);
    assert_eq!(
        one_transfer.frontiers()[0].reason,
        FrontierReason::Budget(Limit::Transfers)
    );
    let entry_work = events
        .try_iter()
        .find_map(|event| match event {
            Event::Execution {
                transfers: 0, work, ..
            } => Some(work),
            _ => None,
        })
        .unwrap();
    assert!(entry_work > config.max_work);
    let one_transfer_work = one_transfer.work();
    assert!(one_transfer_work > entry_work);
    let mut funded_config = config.clone();
    funded_config.max_work = one_transfer_work;
    let graph = native::run(
        &native::product(),
        AnalysisInput::new(admission.world().clone(), admission.entry().clone()).unwrap(),
        funded_config.clone(),
        &Control::default(),
    )
    .unwrap();
    assert!(graph.transfers() > 0);
    assert!(
        graph
            .states()
            .iter()
            .any(|state| state.active().jump_history.is_empty() && state.executed_pcs == [0, 1, 3])
    );
    assert!(graph.edges().iter().any(|edge| {
        edge.kind == analysis::MachineEdgeKind::Intraprocedural(EdgeKind::Jump)
            && graph.states()[edge.from].active().jump_history.is_empty()
            && graph.states()[edge.to].active().jump_history == [0]
    }));
    for (graph, config) in [(&admission, &config), (&graph, &funded_config)] {
        assert_eq!(graph.config().analysis.context_depth, usize::MAX);
        assert!(!graph.config().use_summaries);
        assert!(graph.config().root_terminal_coverage);
        assert_eq!(
            serde_json::to_value(graph.config()).unwrap(),
            serde_json::to_value(config).unwrap()
        );
        assert_eq!(graph.status(), Status::Incomplete);
        assert!(graph.work() <= config.max_work);
        assert!(graph.work() > 0);
        assert!(graph.states().len() < config.analysis.max_states);
        assert!(graph.transfers() < config.analysis.max_transfers);
        for state in graph.states() {
            assert_eq!(state.key.frames.len(), 1);
            assert_eq!(state.active().basic_block_index, 0);
            assert!(state.active().jump_history.is_empty() || state.active().jump_history == [0]);
        }
        assert!(
            graph
                .frontiers()
                .iter()
                .any(|frontier| frontier.reason == FrontierReason::Work)
        );
        assert!(matches!(
            ssa::build_world(graph),
            Err(ssa::SsaError::IncompleteAnalysis)
        ));
    }
    let coverage = graph.root_terminal_coverage().unwrap();
    assert_eq!(
        coverage.scope(),
        analysis::RootTerminalScope::AllInputStates
    );
    assert_eq!(coverage.trigger(), &analysis::RootTerminalTrigger::Work);
    assert_eq!(
        serde_json::to_value(coverage.input().config()).unwrap(),
        serde_json::to_value(&funded_config).unwrap()
    );
    assert_eq!(
        serde_json::to_value(coverage.input().entry()).unwrap(),
        serde_json::to_value(admission.entry()).unwrap()
    );
    assert_eq!(
        coverage.input().initial_store(),
        &graph.states()[0].entry.store
    );
    assert_eq!(coverage.input().call_value().singleton(), Some(U256::ZERO));
    assert_eq!(
        coverage
            .input()
            .calldata()
            .exact_bytes(native::product().borrowed()),
        Some(Vec::new())
    );
    eprintln!(
        "5k admission work={}, entry/input checkpoint={entry_work}, one-transfer receipt={one_transfer_work}, funded work={}, transfers={}",
        admission.work(),
        graph.work(),
        graph.transfers()
    );

    let chain = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(jump_chain(12, None), Default::default())
            .unwrap(),
        ExecutionConfig {
            analysis: Config {
                context_depth: usize::MAX,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    assert_eq!(chain.config().context_depth, usize::MAX);
    assert_eq!(chain.status(), Status::Converged);
    assert!(chain.frontiers().is_empty());
    let pcs: Vec<_> = chain
        .program()
        .blocks()
        .iter()
        .map(|block| block.start_pc)
        .collect();
    assert_distinct_contexts(&chain, usize::MAX);
    assert_jump_trace(&chain, &pcs, usize::MAX);
    assert_eq!(
        state_after_sources(&chain, pcs[12], &pcs[..12], usize::MAX)
            .key
            .context
            .len(),
        12
    );
    ssa::build(&chain).unwrap().verify(&chain).unwrap();
}

#[test]
fn finite_internal_calls_accept_depth_ten_and_maximum_usize() {
    let code = include_str!("../../../examples/internal-calls.hex");
    let program = Program::from_hex(code).unwrap();
    assert_eq!(
        program
            .blocks()
            .iter()
            .map(|block| block.start_pc)
            .collect::<Vec<_>>(),
        [0, 5, 12, 14]
    );
    for (source, return_pc) in [(0, 5), (1, 12)] {
        let block = &program.blocks()[source];
        let pushes: Vec<_> = block
            .instructions
            .iter()
            .filter_map(|i| i.immediate)
            .collect();
        assert_eq!(pushes, [U256::from(return_pc), U256::from(14)]);
        assert!(program.jumpdest_blocks().contains_key(&return_pc));
        assert_eq!(block.instructions.last().unwrap().opcode, opcode::JUMP);
    }
    assert_eq!(
        program.blocks()[3]
            .instructions
            .iter()
            .map(|i| i.opcode)
            .collect::<Vec<_>>(),
        [opcode::JUMPDEST, opcode::PUSH1, opcode::SWAP1, opcode::JUMP]
    );
    assert_eq!(program.jumpdest_blocks().get(&14), Some(&3));
    for depth in [10, usize::MAX] {
        let graph = native::run(
            &native::product(),
            AnalysisInput::<NumericValue>::from_program(program.clone(), Default::default())
                .unwrap(),
            ExecutionConfig {
                analysis: Config {
                    context_depth: depth,
                    ..Config::default()
                },
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
        .unwrap();
        assert_eq!(graph.config().context_depth, depth);
        assert_eq!(graph.status(), Status::Converged);
        assert!(graph.frontiers().is_empty());
        assert_distinct_contexts(&graph, depth);
        let trace = assert_jump_trace(&graph, &[0, 14, 5, 14, 12], depth);
        assert_eq!(latest_distinct(&[0, 14, 5, 14], depth), [0, 5, 14]);
        assert_eq!(graph.states()[trace[4]].key.context, [0, 5, 14]);
        let expected: [&[u64]; 5] = [&[], &[5], &[42], &[12], &[42]];
        for (id, values) in trace.iter().zip(expected) {
            let state = &graph.states()[*id];
            assert_eq!(state.key.stack_height, values.len());
            assert_eq!(
                state
                    .entry_stack
                    .iter()
                    .map(EvmValue::<NumericValue>::singleton)
                    .collect::<Vec<_>>(),
                values
                    .iter()
                    .map(|value| Some(U256::from(*value)))
                    .collect::<Vec<_>>()
            );
        }
        let ssa = ssa::build(&graph).unwrap();
        ssa.verify(&graph).unwrap();
        for pair in trace.windows(2) {
            let from = &graph.states()[pair[0]];
            let to = &graph.states()[pair[1]];
            assert_eq!(from.exit_stack, to.entry_stack);
            let from_ssa = ssa
                .blocks()
                .iter()
                .find(|block| block.state == from.id)
                .unwrap();
            let to_ssa = ssa
                .blocks()
                .iter()
                .find(|block| block.state == to.id)
                .unwrap();
            let jump = from_ssa.instructions.last().unwrap();
            assert_eq!(jump.opcode, opcode::JUMP);
            assert_eq!(jump.operands.len(), 1);
            if graph.program().blocks()[from.key.basic_block_index].start_pc == 14 {
                assert_eq!(jump.operands, [from_ssa.phis[0].result]);
                let returned = from_ssa.instructions.iter().find(|i| i.pc == 15).unwrap();
                assert_eq!(returned.immediate, Some(U256::from(42)));
                assert_eq!(from_ssa.exit_stack, returned.results);
            } else {
                let pushes: Vec<_> = from_ssa
                    .instructions
                    .iter()
                    .filter(|i| i.opcode == opcode::PUSH1)
                    .collect();
                assert_eq!(jump.operands, pushes[1].results);
                assert_eq!(from_ssa.exit_stack, pushes[0].results);
            }
            assert_eq!(to_ssa.phis.len(), to.entry_stack.len());
            for phi in &to_ssa.phis {
                assert!(phi.inputs.iter().any(|input| input.predecessor == from.id
                    && input.value == from_ssa.exit_stack[phi.slot]));
            }
        }
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
