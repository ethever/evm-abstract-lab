//! Retained hash guards preserve branches; optional scalar loss stays diagnostic.

use alloy_primitives::Address;
use evm_abstract::{
    Fork,
    analysis::{
        self, DiagnosticKind, EdgeKind, ExecutionConfig, FrontierReason, InstructionProgress,
        MachineEdgeKind, OutcomeKind, Status,
    },
    domain::{
        AbstractValue,
        relational::{Constraint, QueryReason},
        symbolic::ExprKind,
    },
    ssa::{self, SsaError},
    world::{Account, ByteArray, Entry, World},
};
use revm_bytecode::opcode;

#[test]
fn copied_unknown_hash_branch_covers_both_retained_alternatives() {
    let address = Address::repeat_byte(0x11);
    let mut world = World::offline(Fork::Osaka, "guard-limit", "unknown calldata");
    // Copy 64 unknown calldata bytes, hash them and branch on the hash.
    // Both edges terminate normally; no concrete caller/content is supplied.
    let account = Account::from_hex("60405f5f3760405f20600d57005b00", Fork::Osaka).unwrap();
    world.insert(address, account).unwrap();
    let mut config = ExecutionConfig {
        max_work: 1_000_000_000,
        max_memory_bytes: 64,
        ..ExecutionConfig::default()
    };
    config.analysis.relations.max_nodes = 64;
    let result = analysis::analyze_world(world, Entry::new(address), config).unwrap();
    assert_eq!(result.status(), Status::Converged);
    assert!(result.frontiers().is_empty());
    assert_eq!(
        result
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .count(),
        2,
        "both retained hash alternatives terminate normally"
    );
    let branches = result
        .edges()
        .iter()
        .filter_map(|edge| match edge.kind {
            MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue) => Some((edge.to, true)),
            MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse) => Some((edge.to, false)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(branches.len(), 2);
    let mut retained_guards = Vec::new();
    for (target, nonzero) in branches {
        let guard = result.states()[target]
            .entry
            .relations
            .constraints()
            .find_map(|c| match c {
                Constraint::Truth {
                    expression,
                    nonzero: retained,
                } if *retained == nonzero
                    && matches!(expression.kind(), ExprKind::Keccak { .. }) =>
                {
                    Some(expression.clone())
                }
                _ => None,
            })
            .expect("each branch retains the actual unknown-content hash condition");
        retained_guards.push(guard);
    }
    assert_eq!(retained_guards[0], retained_guards[1]);
}

#[test]
fn optional_expression_refusal_preserves_completion_and_a_typed_work_frontier() {
    let address = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "optional-expression-limit");
    world
        .insert(
            address,
            Account::from_hex("3460010100", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let mut entry = Entry::new(address);
    entry.environment.caller = Address::repeat_byte(0x90).into();
    entry.environment.value = AbstractValue::top();
    entry.environment.calldata = ByteArray::empty();
    let mut config = ExecutionConfig {
        // complete.work() - 1 must isolate the mandatory STOP finalization fee.
        root_terminal_coverage: false,
        use_summaries: false,
        ..ExecutionConfig::default()
    };
    config.analysis.context_depth = 0;
    config.analysis.relations.max_nodes = 2;
    // The input roots fit; CALLVALUE + 1 exceeds only optional scalar retention.
    let complete = analysis::analyze_world(world.clone(), entry.clone(), config.clone()).unwrap();
    assert_eq!(complete.status(), Status::Converged);
    assert!(complete.frontiers().is_empty());
    assert!(
        complete
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return)
    );
    ssa::build_world(&complete)
        .unwrap()
        .verify(&complete)
        .unwrap();

    // This acyclic leaf's last mandatory charge finalizes STOP. Refusing it
    // leaves a real Work frontier in addition to ADD's precision diagnostic.
    config.max_work = complete.work().checked_sub(1).unwrap();
    let work_limit = config.max_work;
    let refused = analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(refused.status(), Status::Incomplete);
    assert!(refused.work() <= work_limit);
    let [frontier] = refused.frontiers() else {
        panic!("expected exactly the refused STOP finalization")
    };
    assert_eq!(frontier.reason, FrontierReason::Work);
    assert_eq!((frontier.from, frontier.pc), (Some(0), Some(4)));
    assert!(
        refused
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind == OutcomeKind::Failure)
    );
    assert!(matches!(
        ssa::build_world(&refused),
        Err(SsaError::IncompleteAnalysis)
    ));

    for result in [&complete, &refused] {
        assert!(result.diagnostics().iter().any(|diagnostic| {
            diagnostic.state == 0
                && diagnostic.pc == 3
                && diagnostic.kind
                    == DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit)
        }));
        let state = &result.states()[0];
        assert_eq!(state.executed_pcs, [0, 1, 3, 4]);
        assert_eq!(state.exit_stack.len(), 1);
        assert!(state.exit_stack[0].numeric().is_top());
        let evidence = state.execution_evidence().unwrap();
        assert!(evidence.is_current());
        let values = evidence.instruction_values().unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values[2].instruction_index(), 2);
        assert!(values[2].numeric().is_top());
        let ir = ssa::build_partial_world(result).unwrap();
        let block = &ir.blocks()[0];
        let addition = block
            .instructions
            .iter()
            .find(|step| step.instruction.opcode == opcode::ADD)
            .unwrap();
        assert_eq!(addition.instruction.pc, 3);
        assert_eq!(addition.progress, InstructionProgress::Completed);
        assert_eq!(addition.instruction.results.len(), 1);
        assert_eq!(block.exit_frames[0], addition.instruction.results);
        let stop = block.instructions.last().unwrap();
        assert_eq!(
            (stop.instruction.pc, stop.instruction.opcode),
            (4, opcode::STOP)
        );
        assert_eq!(stop.progress, InstructionProgress::Dispatched);
        assert!(stop.effect_result.is_some());
        assert_eq!(block.exit_effect, stop.effect_result.unwrap());
        assert_eq!(ir.frontiers().len(), result.frontiers().len());
        ir.verify(result).unwrap();
    }
}
