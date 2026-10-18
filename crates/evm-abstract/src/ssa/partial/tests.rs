use super::{PartialBlockCoverage, build_partial_world};
use crate::{
    Address, Fork, U256,
    analysis::{
        self, DiagnosticKind, ExecutionConfig, FrameCode, FrontierReason, InstructionProgress,
        MachineEdgeKind, OutcomeKind, Status, WorldAnalysis,
    },
    domain::{AbstractValue, relational::QueryReason},
    ssa::{self, SsaError, SsaInvariantKind as Kind},
    world::{Account, ByteArray, Entry, EvmEnvironment, World},
};
use revm_bytecode::opcode;

fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

fn config() -> ExecutionConfig {
    let mut config = ExecutionConfig::default();
    config.analysis.context_depth = 0;
    config.analysis.relations.enabled = false;
    config.use_summaries = false;
    config
}

fn run(code: &str, config: ExecutionConfig) -> WorldAnalysis {
    run_with_value(code, config, AbstractValue::constant(U256::ZERO))
}

fn run_with_value(code: &str, config: ExecutionConfig, value: AbstractValue) -> WorldAnalysis {
    let mut world = World::new(Fork::Osaka, "partial SSA execution phases");
    world
        .insert(
            address(0x101),
            Account::from_hex(code, Fork::Osaka).unwrap(),
        )
        .unwrap();
    analysis::analyze_world(
        world,
        Entry {
            address: address(0x101),
            environment: EvmEnvironment {
                to: address(0x101).into(),
                caller: address(0x900).into(),
                value,
                calldata: ByteArray::empty(),
                ..EvmEnvironment::default()
            },
        },
        config,
    )
    .unwrap()
}

fn pending_mload() -> WorldAnalysis {
    const CODE: &str = "602a60405100";
    let mut config = config();
    // Bisect the detailed MLOAD receipt without optional preparation fees.
    config.root_terminal_coverage = false;
    config.max_memory_bytes = 32;
    let complete = run(CODE, config.clone());
    assert_eq!(complete.status(), Status::Converged);
    assert_eq!(complete.states()[0].executed_pcs, [0, 2, 4, 5]);
    // In this acyclic prefix, the first budget admitting MLOAD's visit pays
    // for operand consumption but cannot also pay its positive read-scan fee.
    // Derive that cutoff from actual work, including root admission fees.
    let (mut minimum, mut maximum) = (1, complete.work());
    while minimum < maximum {
        let midpoint = minimum + (maximum - minimum) / 2;
        config.max_work = midpoint;
        let candidate = run(CODE, config.clone());
        if candidate
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&4))
        {
            maximum = midpoint;
        } else {
            minimum = midpoint + 1;
        }
    }
    config.max_work = minimum;
    let analysis = run(CODE, config);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert_eq!(analysis.states()[0].executed_pcs, [0, 2, 4]);
    let [frontier] = analysis.frontiers() else {
        panic!("expected exactly the refused MLOAD read scan")
    };
    assert_eq!(frontier.reason, FrontierReason::Work);
    assert_eq!((frontier.from, frontier.pc), (Some(0), Some(4)));
    assert_eq!(analysis.outcomes().len(), 1);
    analysis
}

fn unknown_call() -> WorldAnalysis {
    let mut world = World::new(Fork::Osaka, "partial unknown call with covered candidates");
    world
        .insert(
            address(0x101),
            Account::from_hex("5f5f5f5f5f345af100", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world.insert(address(0x200), Account::empty()).unwrap();
    world.insert(address(4), Account::empty()).unwrap();
    // Keep a genuine uncovered effect: this target can attempt creation with
    // unresolved destination facts. Unknown targets themselves are summarized.
    world
        .insert(
            address(0x300),
            Account::from_hex("365f5ff000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let mut config = config();
    config.max_call_depth = 3;
    analysis::analyze_world(
        world,
        Entry {
            address: address(0x101),
            environment: EvmEnvironment {
                to: address(0x101).into(),
                caller: address(0x900).into(),
                value: AbstractValue::top(),
                calldata: ByteArray::empty(),
                ..EvmEnvironment::default()
            },
        },
        config,
    )
    .unwrap()
}

#[test]
fn unknown_call_keeps_frontiers_known_candidates_and_deferred_results() {
    let analysis = unknown_call();
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis
            .frontiers()
            .iter()
            .any(|f| matches!(f.reason, FrontierReason::Creation(_)))
    );
    let ir = build_partial_world(&analysis).unwrap();
    assert_eq!(ir.status(), Status::Incomplete);
    assert_eq!(ir.frontiers().len(), analysis.frontiers().len());
    assert!(
        ir.transitions()
            .iter()
            .any(|t| t.kind == MachineEdgeKind::Call)
    );
    assert!(
        ir.transitions()
            .iter()
            .any(|t| t.kind == MachineEdgeKind::Return)
    );
    assert!(
        ir.blocks()
            .iter()
            .any(|b| analysis.states()[b.state].active().mode == FrameCode::Empty)
    );
    assert!(ir.blocks().iter().any(|b| matches!(
        analysis.states()[b.state].active().mode,
        FrameCode::Precompile(_)
    )));
    let call = ir
        .blocks()
        .iter()
        .flat_map(|b| &b.instructions)
        .find(|s| s.instruction.opcode == opcode::CALL)
        .unwrap();
    assert_eq!(call.progress, InstructionProgress::Dispatched);
    assert_eq!(call.instruction.operands.len(), 7);
    assert!(call.instruction.results.is_empty());
    assert!(
        ir.transitions()
            .iter()
            .filter(|t| t.kind == MachineEdgeKind::Call)
            .all(|t| t.result.is_none())
    );
    assert!(ir.blocks().iter().all(|b| !b.incoming_complete));
    assert!(matches!(
        ssa::build_world(&analysis),
        Err(SsaError::IncompleteAnalysis)
    ));
}

#[test]
fn pending_mload_consumes_its_offset_without_inventing_the_loaded_word() {
    let analysis = pending_mload();
    assert_eq!(analysis.status(), Status::Incomplete);
    let ir = build_partial_world(&analysis).unwrap();
    let block = &ir.blocks()[0];
    assert_eq!(block.coverage, PartialBlockCoverage::Current);
    let pending = block
        .instructions
        .iter()
        .find(|step| step.instruction.pc == 4)
        .unwrap();
    assert_eq!(pending.instruction.opcode, opcode::MLOAD);
    assert_eq!(pending.progress, InstructionProgress::OperandsConsumed);
    assert_eq!(pending.instruction.operands.len(), 1);
    assert!(pending.instruction.results.is_empty());
    assert!(pending.effect_result.is_some());
    assert_eq!(block.exit_effect, pending.effect_result.unwrap());
    assert_eq!(pending.instruction.operands, vec![1]);
    assert_eq!(block.exit_frames[0], vec![0]);
    assert_eq!(block.exit_frames[0].len(), 1);
    assert_eq!(ir.value_count(), 2);
    assert_eq!(
        analysis.states()[0]
            .execution_evidence()
            .unwrap()
            .instruction_values()
            .unwrap()
            .iter()
            .map(|value| (value.instruction_index(), value.numeric().singleton()))
            .collect::<Vec<_>>(),
        [(0, Some(U256::from(42))), (1, Some(U256::from(64)))]
    );
    assert_eq!(ir.frontiers().len(), 1);
    assert_eq!(ir.frontiers()[0].reason, FrontierReason::Work);
    assert_eq!(ir.frontiers()[0].pc, Some(4));
    assert!(
        analysis
            .outcomes()
            .iter()
            .all(|o| o.kind == OutcomeKind::Failure)
    );
    ir.verify(&analysis).unwrap();
}

#[test]
fn legal_logical_mload_beyond_the_host_cap_completes_with_a_real_result() {
    let mut config = config();
    config.max_memory_bytes = 32;
    let analysis = run("602a60405100", config);
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    let state = &analysis.states()[0];
    assert_eq!(state.executed_pcs, [0, 2, 4, 5]);
    assert_eq!(state.exit_stack.len(), 2);
    assert_eq!(state.exit_stack[0].singleton(), Some(U256::from(42)));
    assert_eq!(state.exit_stack[1].singleton(), Some(U256::ZERO));
    let values = state
        .execution_evidence()
        .unwrap()
        .instruction_values()
        .unwrap();
    assert_eq!(values.len(), 3);
    assert_eq!(values[2].instruction_index(), 2);
    assert_eq!(values[2].numeric().singleton(), Some(U256::ZERO));
    assert_eq!(
        state
            .exit
            .as_ref()
            .unwrap()
            .active()
            .memory
            .len()
            .singleton(),
        Some(U256::from(96))
    );
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|o| o.kind == OutcomeKind::Return)
    );
    let full = ssa::build_world(&analysis).unwrap();
    let partial = build_partial_world(&analysis).unwrap();
    let load = &partial.blocks()[0].instructions[2];
    assert_eq!(load.instruction.pc, 4);
    assert_eq!(load.instruction.opcode, opcode::MLOAD);
    assert_eq!(load.progress, InstructionProgress::Completed);
    assert_eq!(load.instruction.operands, vec![1]);
    assert_eq!(load.instruction.results, vec![2]);
    assert_eq!(partial.blocks()[0].exit_frames[0], vec![0, 2]);
    assert_eq!(partial.value_count(), 3);
    assert_eq!(partial.value_count(), full.value_count());
    assert_eq!(partial.effect_count(), full.effect_count());
    full.verify(&analysis).unwrap();
    partial.verify(&analysis).unwrap();
}

#[test]
fn joins_after_the_last_transfer_leave_a_stale_body_and_deferred_old_edge() {
    let mut config = config();
    config.analysis.max_transfers = 2;
    let analysis = run("5f5b600101600156", config);
    assert_eq!(analysis.status(), Status::Incomplete);
    let state = analysis
        .states()
        .iter()
        .find(|s| s.execution_evidence().is_some_and(|e| !e.is_current()))
        .expect("the loop entry must grow after its first transfer");
    assert!(!state.executed_pcs.is_empty());
    let ir = build_partial_world(&analysis).unwrap();
    let stale = &ir.blocks()[state.id];
    assert_eq!(stale.coverage, PartialBlockCoverage::Stale);
    assert!(stale.instructions.is_empty());
    assert!(ir.deferred_edges().iter().any(|edge| edge.from == state.id));
    assert!(!stale.open_incoming.is_empty());
}

#[test]
fn budget_pending_state_has_entry_parameters_and_no_unexecuted_body() {
    let mut config = config();
    config.analysis.max_transfers = 1;
    let analysis = run("5f5b600101600156", config);
    let ir = build_partial_world(&analysis).unwrap();
    let pending = ir
        .blocks()
        .iter()
        .find(|b| b.coverage == PartialBlockCoverage::Unexecuted)
        .unwrap();
    assert_eq!(pending.phis.len(), 1);
    assert!(pending.instructions.is_empty());
    assert_eq!(pending.exit_frames[0], vec![pending.phis[0].result]);
}

#[test]
fn a_fresh_shorter_retry_does_not_reuse_an_older_completed_backedge() {
    let mut first_config = config();
    // Measure the original detailed retry ledger for the stale-edge sweep.
    first_config.root_terminal_coverage = false;
    first_config.analysis.max_transfers = 2;
    let first = run("5f5b600101600156", first_config);
    let base_work = first.work();
    let step = (base_work / 512).max(1);
    let witness = (0..=base_work.saturating_mul(2))
        .step_by(step)
        .find_map(|extra| {
            let mut config = config();
            config.root_terminal_coverage = false;
            config.max_work = base_work.saturating_add(extra);
            let analysis = run("5f5b600101600156", config);
            let prefix = analysis
                .states()
                .iter()
                .find(|state| {
                    state.execution_evidence().is_some_and(|evidence| {
                        evidence.is_current()
                            && evidence.successors().is_empty()
                            && !state.executed_pcs.is_empty()
                            && state.executed_pcs.last() != Some(&7)
                    }) && analysis
                        .edges()
                        .iter()
                        .any(|edge| edge.from == state.id && edge.to == state.id)
                })
                .map(|state| state.id);
            prefix.map(|id| (analysis, id))
        })
        .expect("a work frontier must stop a current retry before its old JUMP");
    let (analysis, state) = witness;
    let ir = build_partial_world(&analysis).unwrap();
    assert_eq!(ir.blocks()[state].coverage, PartialBlockCoverage::Current);
    assert!(
        ir.blocks()[state]
            .instructions
            .last()
            .unwrap()
            .instruction
            .pc
            != 7
    );
    assert!(ir.deferred_edges().iter().any(|edge| {
        edge.from == state
            && edge.to == state
            && edge.reason == super::DeferredEdgeReason::NotInExecutionEvidence
    }));
    assert!(
        !ir.transitions()
            .iter()
            .any(|transition| analysis.edges()[transition.edge].from == state)
    );
}

#[test]
fn completed_fault_has_no_fabricated_operands_or_results() {
    let analysis = run("fe", config());
    assert_eq!(analysis.status(), Status::Converged);
    let ir = build_partial_world(&analysis).unwrap();
    let fault = &ir.blocks()[0].instructions[0];
    assert_eq!(fault.progress, InstructionProgress::Faulted);
    assert!(fault.instruction.fault);
    assert!(fault.instruction.operands.is_empty());
    assert!(fault.instruction.results.is_empty());
}

#[test]
fn optional_expression_limit_preserves_completion_and_a_separate_real_work_frontier() {
    let mut config = config();
    // complete.work() - 1 must isolate the mandatory STOP finalization fee.
    config.root_terminal_coverage = false;
    config.analysis.relations.enabled = true;
    config.analysis.relations.max_nodes = 2;
    // CALLVALUE provides an unknown word with an input leaf. The bound admits
    // initial state roots, while adding 1 exceeds optional term retention.
    let complete = run_with_value("3460010100", config.clone(), AbstractValue::top());
    assert_eq!(complete.status(), Status::Converged);
    assert!(complete.frontiers().is_empty());
    assert!(
        complete
            .outcomes()
            .iter()
            .any(|o| o.kind == OutcomeKind::Return)
    );
    ssa::build_world(&complete)
        .unwrap()
        .verify(&complete)
        .unwrap();

    // Refuse the last root-finalization charge, after ADD and STOP. This is a
    // genuine coverage gap, separately from ADD's optional precision diagnostic.
    config.max_work = complete.work().checked_sub(1).unwrap();
    let refused = run_with_value("3460010100", config, AbstractValue::top());
    assert_eq!(refused.status(), Status::Incomplete);
    let [frontier] = refused.frontiers() else {
        panic!("expected exactly the refused STOP finalization")
    };
    assert_eq!(frontier.reason, FrontierReason::Work);
    assert_eq!((frontier.from, frontier.pc), (Some(0), Some(4)));
    assert!(
        refused
            .outcomes()
            .iter()
            .all(|o| o.kind == OutcomeKind::Failure)
    );
    assert!(matches!(
        ssa::build_world(&refused),
        Err(SsaError::IncompleteAnalysis)
    ));

    for analysis in [&complete, &refused] {
        assert!(analysis.diagnostics().iter().any(|diagnostic| {
            diagnostic.state == 0
                && diagnostic.pc == 3
                && diagnostic.kind
                    == DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit)
        }));
        let state = &analysis.states()[0];
        assert_eq!(state.executed_pcs, [0, 1, 3, 4]);
        assert_eq!(state.exit_stack.len(), 1);
        assert!(state.exit_stack[0].numeric().is_top());
        let values = state
            .execution_evidence()
            .unwrap()
            .instruction_values()
            .unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values[2].instruction_index(), 2);
        assert!(values[2].numeric().is_top());
        let ir = build_partial_world(analysis).unwrap();
        let block = &ir.blocks()[0];
        let addition = block
            .instructions
            .iter()
            .find(|step| step.instruction.opcode == opcode::ADD)
            .unwrap();
        assert_eq!(addition.progress, InstructionProgress::Completed);
        assert_eq!(addition.instruction.results.len(), 1);
        assert_eq!(block.exit_frames[0], addition.instruction.results);
        let stop = block.instructions.last().unwrap();
        assert_eq!(stop.instruction.opcode, opcode::STOP);
        assert_eq!(stop.progress, InstructionProgress::Dispatched);
        assert!(stop.effect_result.is_some());
        assert_eq!(block.exit_effect, stop.effect_result.unwrap());
        assert_eq!(ir.frontiers().len(), analysis.frontiers().len());
        ir.verify(analysis).unwrap();
    }
}

#[test]
fn partial_verifier_rejects_phase_result_opcode_and_operand_tampering() {
    let analysis = pending_mload();
    let ir = build_partial_world(&analysis).unwrap();
    let pending_index = ir.blocks[0]
        .instructions
        .iter()
        .position(|step| step.instruction.pc == 4)
        .unwrap();
    let pending = &ir.blocks[0].instructions[pending_index];
    assert_eq!(pending.instruction.opcode, opcode::MLOAD);
    assert_eq!(pending.progress, InstructionProgress::OperandsConsumed);
    assert_eq!(pending.instruction.operands.len(), 1);
    assert!(pending.instruction.results.is_empty());
    let mut phase = ir.clone();
    phase.blocks[0].instructions[pending_index].progress = InstructionProgress::Completed;
    let SsaError::Invariant(error) = phase.verify(&analysis).unwrap_err() else {
        panic!("expected invariant")
    };
    assert_eq!(error.kind, Kind::PartialInstructionIdentity);
    assert_eq!(error.state, Some(0));
    assert_eq!(error.pc, Some(4));
    let mut result = ir.clone();
    result.blocks[0].instructions[pending_index]
        .instruction
        .results
        .push(0);
    assert!(result.verify(&analysis).is_err());
    let mut opcode = ir.clone();
    opcode.blocks[0].instructions[pending_index]
        .instruction
        .opcode = opcode::ADD;
    assert!(opcode.verify(&analysis).is_err());
    let mut operand = ir.clone();
    operand.blocks[0].instructions[pending_index]
        .instruction
        .operands[0] = ir.value_count();
    assert!(operand.verify(&analysis).is_err());
}

#[test]
fn partial_verifier_rejects_lost_frontiers_and_broken_effects() {
    let analysis = unknown_call();
    let ir = build_partial_world(&analysis).unwrap();
    let mut frontiers = ir.clone();
    frontiers.frontiers.clear();
    assert!(frontiers.verify(&analysis).is_err());
    let mut effects = ir.clone();
    effects.blocks[0].instructions[0].effect_input = ir.effect_count();
    let SsaError::Invariant(error) = effects.verify(&analysis).unwrap_err() else {
        panic!("expected invariant")
    };
    assert_eq!(error.kind, Kind::PartialInstructionEffectChain);
    assert_eq!((error.state, error.pc), (Some(0), Some(0)));
    assert_eq!(error.expected, Some(ir.blocks[0].effect.result));
    assert_eq!(error.observed, Some(ir.effect_count()));
    let mut transition = ir.clone();
    transition
        .transitions
        .iter_mut()
        .find(|t| t.kind == MachineEdgeKind::Call)
        .unwrap()
        .result = Some(0);
    assert!(transition.verify(&analysis).is_err());
}

#[test]
fn partial_verifier_rejects_omitted_deferred_edges_and_false_closed_phi_coverage() {
    let mut config = config();
    config.analysis.max_transfers = 2;
    let analysis = run("5f5b600101600156", config);
    let ir = build_partial_world(&analysis).unwrap();
    let mut edges = ir.clone();
    edges.deferred_edges.clear();
    assert!(edges.verify(&analysis).is_err());
    let mut coverage = ir.clone();
    coverage.blocks[0].incoming_complete = true;
    assert!(coverage.verify(&analysis).is_err());
    let mut stale = ir.clone();
    stale
        .blocks
        .iter_mut()
        .find(|b| b.coverage == PartialBlockCoverage::Stale)
        .unwrap()
        .coverage = PartialBlockCoverage::Current;
    assert!(stale.verify(&analysis).is_err());
}

#[test]
fn strict_verifier_still_rejects_an_incomplete_analysis() {
    let mut analysis = run("00", config());
    let full = ssa::build_world(&analysis).unwrap();
    analysis.status = Status::Incomplete;
    assert!(matches!(
        full.verify(&analysis),
        Err(SsaError::IncompleteAnalysis)
    ));
}

#[test]
fn closed_prefixes_keep_the_complete_ssa_definitions_and_edge_namespace() {
    let analysis = run("600260030100", config());
    let full = ssa::build_world(&analysis).unwrap();
    let partial = build_partial_world(&analysis).unwrap();
    assert_eq!(partial.status(), Status::Converged);
    assert!(partial.deferred_edges().is_empty());
    assert!(partial.frontiers().is_empty());
    assert_eq!(partial.value_count(), full.value_count());
    assert_eq!(partial.effect_count(), full.effect_count());
    for (recorded, complete) in partial.blocks()[0]
        .instructions
        .iter()
        .zip(&full.blocks()[0].instructions)
    {
        assert_eq!(recorded.instruction.opcode, complete.opcode);
        assert_eq!(recorded.instruction.operands, complete.operands);
        assert_eq!(recorded.instruction.results, complete.results);
    }
    assert_eq!(
        partial.blocks()[0].exit_frames,
        full.blocks()[0].exit_frames
    );
}

#[test]
fn empty_input_frontier_does_not_invent_a_root_block() {
    let mut config = config();
    config.max_work = 1;
    let analysis = run("00", config);
    assert!(analysis.states().is_empty());
    let ir = build_partial_world(&analysis).unwrap();
    assert_eq!(ir.status(), Status::Incomplete);
    assert!(ir.blocks().is_empty());
    assert_eq!(ir.value_count(), 0);
    assert_eq!(ir.effect_count(), 0);
    assert!(!ir.frontiers().is_empty());
}
