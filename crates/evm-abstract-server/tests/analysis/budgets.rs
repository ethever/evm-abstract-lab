//! Independent budget boundaries: work, representation and entry rollback
//! have different committed prefixes and must retain separate assertions.
use super::*;
use alloy_primitives::U256;
use evm_abstract::domain::symbolic::{ByteStateKind, ExprId, ExprLimits};
use evm_abstract_protocol::{
    AddressValue, AnalysisInput, Expression, FrontierDetails, InputSymbol, InputSymbolKind,
    SsaFactStatus, SsaOperand, SsaOperation, SsaValueOrigin,
};

fn sparse_mstore_code() -> String {
    format!("{}602062100000f3", "602a6210000052".repeat(20))
}

fn sparse_mstore_request() -> AnalyzeRequest {
    // Repeated sparse MSTORE at 1 MiB needs a span beyond the old memory ceiling.
    // The configured finite domain makes this account for >100 million units,
    // while only 32 concrete memory bytes need storing.
    let mut input = request(&sparse_mstore_code());
    input.limits.max_memory_bytes = 1_048_576 + 32;
    // Keep the detailed prefix and refusal controls independent of root preparation.
    input.limits.root_terminal_coverage = false;
    input.limits.max_work = 10_000_000_000;
    input.limits.max_constants = 512;
    input
}

fn returned_word() -> [u8; 32] {
    let mut word = [0; 32];
    word[31] = 42;
    word
}

#[test]
fn sparse_memory_above_the_old_ceiling_returns_the_complete_word() {
    let input = sparse_mstore_request();
    let selected_limits = input.limits.clone();
    let report = analyze(input.clone()).unwrap();
    assert_eq!(
        report.status,
        AnalysisStatus::Converged,
        "{:?}",
        report.frontiers
    );
    assert!(report.ssa.complete);
    assert!(
        report.metadata.work > 100_000_000,
        "work={}",
        report.metadata.work
    );
    assert_eq!(report.metadata.limits, selected_limits);
    let returned = report
        .outcomes
        .iter()
        .find(|outcome| outcome.kind == evm_abstract_protocol::OutcomeKind::Return)
        .unwrap();
    let bytes = &report.byte_arrays[returned.data];
    assert_eq!(bytes.length.constants, Some(vec!["0x20".into()]));
    let word = returned_word();
    assert!(buffer_matches(&report, bytes, &word));
    assert_eq!(bytes.cells.len(), 32);
    let mut wrong = word;
    wrong[15] = 1;
    assert!(!buffer_matches(&report, bytes, &wrong));
    let mut excluded = bytes.clone();
    let value = excluded.cells[31].value;
    excluded.values[value].constants = Some(vec!["0x2b".into()]);
    assert!(!buffer_matches(&report, &excluded, &word));
}

#[test]
fn work_refusal_retains_the_actual_mstore_prefix_and_pending_effect() {
    let input = sparse_mstore_request();
    let code = sparse_mstore_code();
    let word = returned_word();
    // Price changes can move the refusal to a later MSTORE. Require its exact
    // current receipt and committed prefix rather than fixing a guard fee/PC.
    let mut bounded = input.clone();
    bounded.limits.max_work = 100_000_000;
    let work_limits = bounded.limits.clone();
    let partial = analyze(bounded).unwrap();
    assert_eq!(partial.metadata.limits, work_limits);
    assert_eq!(partial.status, AnalysisStatus::Incomplete);
    assert!(!partial.ssa.complete);
    assert!(partial.metadata.work > 0 && partial.metadata.work <= work_limits.max_work);
    let [frontier] = partial.frontiers.as_slice() else {
        panic!(
            "expected one pending MSTORE Work refusal: {:?}",
            partial.frontiers
        )
    };
    assert_eq!(frontier.reason, FrontierDetails::Work);
    assert_eq!(frontier.kind, FrontierKind::Work);
    assert_eq!(frontier.from, Some(0));
    assert!(frontier.range.is_none());
    assert!(frontier.payload.is_none());
    let pc = frontier.pc.expect("Work must name the attempted MSTORE");
    assert!(
        (6..7 * 20).contains(&pc),
        "Work outside the write prefix: {pc}"
    );
    assert_eq!((pc - 6) % 7, 0, "Work must stop at an original MSTORE");
    let completed_writes = (pc - 6) / 7;
    assert!(completed_writes < 20);
    let phase = if completed_writes == 0 {
        "before the first committed MSTORE"
    } else {
        "after a committed MSTORE prefix"
    };
    let [state] = partial.states.as_slice() else {
        panic!("expected only the root state {phase}")
    };
    let [cfg] = partial.cfg.as_slice() else {
        panic!("expected only the root CFG block {phase}")
    };
    let [block] = partial.ssa.blocks.as_slice() else {
        panic!("expected only the root SSA block {phase}")
    };
    assert_eq!(
        (state.state, cfg.id, block.state, cfg.basic_block),
        (0, 0, 0, Some(0))
    );
    assert_eq!(cfg.start_pc, Some(0));
    assert_eq!(cfg.frame_depth, 1);
    assert_eq!(block.coverage, BlockCoverage::Current);
    assert!(!block.incoming_complete);
    assert!(block.phis.is_empty());
    assert!(block.effect_inputs.is_empty());
    assert!(block.open_incoming.is_empty());
    let [entry] = state.entry.frames.as_slice() else {
        panic!("expected the original root frame")
    };
    assert_eq!(entry.basic_block, Some(0));
    assert!(entry.context.is_empty());
    assert!(entry.stack.is_empty());
    assert!(cfg.entry_stack.is_empty());
    assert_eq!(cfg.program, entry.program);
    let program = &partial.programs[entry.program.unwrap()];
    assert_eq!(program.bytecode, format!("0x{code}"));
    assert_eq!(
        program.code_address, None,
        "supplied namespace bytes have no observed physical source"
    );
    assert_eq!(entry.code_hash.as_ref(), Some(&program.code_hash));
    let [source] = program.blocks.as_slice() else {
        panic!("expected one original source block")
    };
    assert_eq!((source.id, source.start_pc), (0, 0));
    assert_eq!(cfg.instructions, source.instructions);
    let prefix: Vec<_> = (0..=completed_writes)
        .flat_map(|write| [7 * write, 7 * write + 2, 7 * write + 6])
        .collect();
    assert_eq!(cfg.executed_pcs, prefix);
    assert_eq!(block.instructions.len(), prefix.len());
    assert_eq!(
        source
            .instructions
            .iter()
            .take_while(|item| item.pc <= pc)
            .map(|item| item.pc)
            .collect::<Vec<_>>(),
        prefix
    );
    let mut effects = vec![block.effect];
    for (index, (step, original)) in block
        .instructions
        .iter()
        .zip(&source.instructions)
        .enumerate()
    {
        assert_eq!((step.pc, step.opcode), (prefix[index], original.opcode));
        assert_eq!(step.immediate, original.immediate);
        assert!(!step.fault);
        assert_eq!(
            step.progress,
            if index + 1 == prefix.len() {
                InstructionProgress::OperandsConsumed
            } else {
                InstructionProgress::Completed
            },
            "instruction {} {phase}",
            step.pc
        );
        assert_eq!(step.effect_input, *effects.last().unwrap());
        let effect = step
            .effect_result
            .expect("each observed step has an effect name");
        assert!(!effects.contains(&effect));
        effects.push(effect);
        if index % 3 == 2 {
            assert_eq!(step.opcode, 0x52);
            assert_eq!(
                step.operands,
                [
                    block.instructions[index - 1].results[0],
                    block.instructions[index - 2].results[0],
                ]
            );
            assert!(step.results.is_empty());
            let SsaOperation::Opcode(operation) = &step.operation else {
                panic!("MSTORE cannot acquire a folded result")
            };
            assert_eq!(operation.opcode, 0x52);
            assert_eq!(
                operation.operands,
                [
                    SsaOperand::Constant("0x100000".into()),
                    SsaOperand::Constant("0x2a".into()),
                ]
            );
            for (operand, constant) in step.operands.iter().zip(["0x100000", "0x2a"]) {
                assert!(partial.ssa.values[*operand].facts.iter().any(|fact| {
                    fact.status == SsaFactStatus::Current
                        && fact.constant.as_deref() == Some(constant)
                }));
            }
        } else {
            assert_eq!(step.opcode, if index % 3 == 0 { 0x60 } else { 0x62 });
            assert!(step.operands.is_empty());
            let [result] = step.results.as_slice() else {
                panic!("every prefix PUSH must retain its original definition")
            };
            let SsaValueOrigin::Instruction(origin) = &partial.ssa.values[*result].origin else {
                panic!("PUSH result must retain its instruction origin")
            };
            assert_eq!(
                (origin.state, origin.instruction_index, origin.pc),
                (0, index, step.pc)
            );
        }
    }
    let pending = block.instructions.last().unwrap();
    assert_eq!((pending.pc, pending.opcode), (pc, 0x52));
    assert_eq!(block.exit_effect, pending.effect_result.unwrap());
    assert_eq!(partial.ssa.effect_count, effects.len());
    assert_eq!(partial.ssa.value_count, 2 * (completed_writes + 1));
    assert_eq!(partial.ssa.values.len(), partial.ssa.value_count);
    assert!(!partial.ssa.values.iter().any(|value| {
        matches!(&value.origin, SsaValueOrigin::Instruction(origin)
            if origin.state == 0 && (origin.pc >= pc || origin.instruction_index >= prefix.len() - 1))
    }));
    assert_eq!(
        block
            .instructions
            .iter()
            .filter(|step| {
                step.opcode == 0x52 && step.progress == InstructionProgress::Completed
            })
            .count(),
        completed_writes
    );
    assert_eq!(block.exit_frames, [Vec::<usize>::new()]);
    assert!(cfg.exit_stack.is_empty());
    assert!(partial.edges.is_empty());
    assert!(partial.ssa.transitions.is_empty());
    assert!(partial.ssa.deferred_edges.is_empty());
    assert!(partial.outcomes.iter().all(|outcome| {
        outcome.state == 0 && outcome.kind == evm_abstract_protocol::OutcomeKind::Failure
    }));
    let exit = state
        .exit
        .as_ref()
        .expect("a current prefix must retain its real state");
    assert_eq!(exit.store, state.entry.store);
    assert_eq!(exit.path, state.entry.path);
    let [frame] = exit.frames.as_slice() else {
        panic!("pending MSTORE must retain the root frame")
    };
    let mut unchanged = entry.clone();
    unchanged.memory = frame.memory;
    assert_eq!(frame, &unchanged);
    let initial_memory = &partial.byte_arrays[entry.memory.unwrap()];
    assert!(initial_memory.memory);
    assert!(buffer_matches(&partial, initial_memory, &[]));
    assert!(initial_memory.cells.is_empty());
    assert_eq!(initial_memory.default.constants, Some(vec!["0x0".into()]));
    assert!(byte_contains(&initial_memory.default, 0));
    let limits = ExprLimits {
        max_nodes: work_limits.max_expression_nodes.try_into().unwrap(),
        max_depth: work_limits.max_expression_depth.try_into().unwrap(),
    };
    let initial = native_view(&partial.symbolic, initial_memory.symbolic.node, limits).unwrap();
    assert_eq!(initial.offset.as_constant(), Some(U256::ZERO));
    assert_eq!(initial.length.as_constant(), Some(U256::ZERO));
    assert!(matches!(initial.state.kind(), ByteStateKind::Zero));
    let Expression::View(initial_view) = &partial.symbolic.nodes[initial_memory.symbolic.node]
    else {
        panic!("entry memory must retain its original view")
    };
    assert!(matches!(
        partial.symbolic.nodes[initial_view.state.node],
        Expression::BytesZero
    ));
    let memory = &partial.byte_arrays[frame.memory.unwrap()];
    assert!(memory.memory);
    assert_eq!(memory.default, initial_memory.default);
    let captured = native_view(&partial.symbolic, memory.symbolic.node, limits).unwrap();
    assert_eq!(captured.offset.as_constant(), Some(U256::ZERO));
    let Expression::View(view) = &partial.symbolic.nodes[memory.symbolic.node] else {
        panic!("pending MSTORE must retain its captured memory view")
    };
    // Identical writes are idempotent in the physical root. Only the complete
    // current receipt above counts writes and distinguishes the pending effect.
    // Check raw structure too: native decoding can normalize equivalent writes.
    if completed_writes == 0 {
        assert_eq!(memory, initial_memory, "{phase}");
        assert_eq!(view.state, initial_view.state);
        assert!(matches!(captured.state.kind(), ByteStateKind::Zero));
        assert!(buffer_matches(&partial, memory, &[]));
    } else {
        assert_eq!(memory.length.constants, Some(vec!["0x100020".into()]));
        assert_eq!(captured.length.as_constant(), Some(U256::from(0x100020)));
        let Expression::WriteWord(write) = &partial.symbolic.nodes[view.state.node] else {
            panic!("committed prefix must retain its raw WriteWord")
        };
        assert_eq!(write.previous, initial_view.state);
        for (operand, constant) in [(write.offset, "0x100000"), (write.value, "0x2a")] {
            assert_eq!(
                partial.symbolic.nodes[operand.node],
                Expression::Constant(constant.into())
            );
        }
        let ByteStateKind::WriteWord {
            previous,
            offset,
            value,
        } = captured.state.kind()
        else {
            panic!("committed prefix must retain exactly its one effective WriteWord")
        };
        assert_eq!(previous, &initial.state);
        assert_eq!(offset.as_constant(), Some(U256::from(0x100000)));
        assert_eq!(value.as_constant(), Some(U256::from(42)));
        // The sole WriteWord over the unchanged zero root proves every unwritten
        // byte is zero. Extract only its full word, under the reported limits.
        let written = captured
            .slice(
                ExprId::constant(U256::from(0x100000)),
                ExprId::constant(U256::from(32)),
                limits.into(),
            )
            .unwrap();
        assert_eq!(
            written.known_bytes(limits).unwrap().as_deref(),
            Some(word.as_slice())
        );
        assert_eq!(memory.cells.len(), 32);
        for (index, cell) in memory.cells.iter().enumerate() {
            assert_eq!(cell.offset, 0x100000 + index);
            assert!(byte_contains(&memory.values[cell.value], word[index]));
        }
    }
    partial.validate().unwrap();
}

#[test]
fn expression_refusal_keeps_expansion_but_does_not_commit_the_first_mstore() {
    let input = sparse_mstore_request();
    // Isolate the mandatory four-node WriteWord from unknown-recipient cleanup:
    // a concrete non-precompile owner leaves RIPEMD definitely untouched, so
    // late_failure does not attempt its unrelated MAY-deletion at block entry.
    // The old smaller memory cap still admits lazy expansion before the refusal.
    let mut bounded = input.clone();
    bounded.limits.max_memory_bytes = 1_048_576;
    bounded.limits.max_expression_nodes = 3;
    let owner = "0x1111111111111111111111111111111111111111";
    let AnalysisInput::Bytecode(source) = &mut bounded.input else {
        panic!("expected the original bytecode input")
    };
    source.address = Some(owner.into());
    let limits = bounded.limits.clone();
    let partial = analyze(bounded).unwrap();
    assert_eq!(partial.metadata.limits, limits);
    assert_eq!(
        partial.metadata.environment.to,
        AddressValue::Concrete(owner.into())
    );
    assert_eq!(partial.status, AnalysisStatus::Incomplete);
    assert!(!partial.ssa.complete);
    let [frontier] = partial.frontiers.as_slice() else {
        panic!(
            "expected only the first MSTORE refusal: {:?}",
            partial.frontiers
        )
    };
    assert_eq!(
        frontier.reason,
        evm_abstract_protocol::FrontierDetails::Relations(
            evm_abstract_protocol::QueryBoundary::ExpressionLimit,
        )
    );
    assert_eq!(frontier.kind, FrontierKind::Relations);
    assert_eq!((frontier.from, frontier.pc), (Some(0), Some(6)));
    assert!(frontier.range.is_none());
    assert!(frontier.payload.is_none());
    let [block] = partial.ssa.blocks.as_slice() else {
        panic!("expected only the root SSA block")
    };
    assert_eq!(block.state, 0);
    assert_eq!(block.coverage, BlockCoverage::Current);
    assert!(!block.incoming_complete);
    assert!(block.phis.is_empty());
    assert!(block.effect_inputs.is_empty());
    assert!(block.open_incoming.is_empty());
    let [value, offset, store] = block.instructions.as_slice() else {
        panic!("expected the two PUSHes and pending first MSTORE")
    };
    for (push, pc, opcode) in [(value, 0, 0x60), (offset, 2, 0x62)] {
        assert_eq!((push.pc, push.opcode), (pc, opcode));
        assert_eq!(push.progress, InstructionProgress::Completed);
        assert!(push.operands.is_empty());
        assert_eq!(push.results.len(), 1);
    }
    let mut effect = block.effect;
    for step in &block.instructions {
        assert!(!step.fault);
        assert_eq!(step.effect_input, effect);
        let next = step
            .effect_result
            .expect("each observed step has an effect");
        assert_ne!(next, effect);
        effect = next;
    }
    assert_eq!(block.exit_effect, effect);
    assert_eq!(partial.ssa.effect_count, 4);
    assert_eq!(partial.ssa.value_count, 2);
    assert_eq!(partial.ssa.values.len(), 2);
    assert_eq!((store.pc, store.opcode), (6, 0x52));
    assert_eq!(store.progress, InstructionProgress::OperandsConsumed);
    assert_eq!(store.operands, [offset.results[0], value.results[0]]);
    for (operand, constant) in store.operands.iter().zip(["0x100000", "0x2a"]) {
        assert!(partial.ssa.values[*operand].facts.iter().any(|fact| {
            fact.status == evm_abstract_protocol::SsaFactStatus::Current
                && fact.constant.as_deref() == Some(constant)
        }));
    }
    assert!(store.results.is_empty());
    assert!(store.effect_result.is_some());
    let SsaOperation::Opcode(operation) = &store.operation else {
        panic!("pending MSTORE cannot acquire a folded result")
    };
    assert_eq!(operation.opcode, 0x52);
    assert_eq!(
        operation.operands,
        [
            SsaOperand::Constant("0x100000".into()),
            SsaOperand::Constant("0x2a".into()),
        ]
    );
    assert_eq!(block.exit_frames, [Vec::<usize>::new()]);
    let [state] = partial.states.as_slice() else {
        panic!("expected only the root state")
    };
    let [cfg] = partial.cfg.as_slice() else {
        panic!("expected only the root CFG block")
    };
    assert_eq!((state.state, cfg.id), (0, 0));
    assert_eq!(cfg.executed_pcs, [0, 2, 6]);
    assert!(cfg.entry_stack.is_empty());
    assert!(cfg.exit_stack.is_empty());
    assert!(partial.edges.is_empty());
    assert!(partial.ssa.transitions.is_empty());
    assert!(partial.ssa.deferred_edges.is_empty());
    let exit = state.exit.as_ref().unwrap();
    assert_eq!(exit.store, state.entry.store);
    assert_eq!(exit.path, state.entry.path);
    let [entry] = state.entry.frames.as_slice() else {
        panic!("expected the original root frame")
    };
    assert_eq!(
        entry.address_value.as_ref(),
        Some(&partial.metadata.environment.to)
    );
    let [frame] = exit.frames.as_slice() else {
        panic!("pending MSTORE must retain the root frame")
    };
    assert!(frame.stack.is_empty());
    let mut unchanged = entry.clone();
    unchanged.memory = frame.memory;
    assert_eq!(frame, &unchanged);
    let initial_memory = &partial.byte_arrays[entry.memory.unwrap()];
    assert!(buffer_matches(&partial, initial_memory, &[]));
    let memory = &partial.byte_arrays[frame.memory.unwrap()];
    assert!(memory.memory);
    assert_eq!(memory.length.constants, Some(vec!["0x100020".into()]));
    assert!(memory.cells.is_empty());
    assert_eq!(memory.default, initial_memory.default);
    assert_eq!(memory.default.constants, Some(vec!["0x0".into()]));
    let Expression::View(view) = &partial.symbolic.nodes[memory.symbolic.node] else {
        panic!("refused write retains memory view")
    };
    assert!(matches!(
        partial.symbolic.nodes[view.state.node],
        Expression::BytesZero
    ));
    let captured = native_view(
        &partial.symbolic,
        memory.symbolic.node,
        ExprLimits {
            max_nodes: limits.max_expression_nodes.try_into().unwrap(),
            max_depth: limits.max_expression_depth.try_into().unwrap(),
        },
    )
    .unwrap();
    assert_eq!(captured.offset.as_constant(), Some(U256::ZERO));
    assert_eq!(captured.length.as_constant(), Some(U256::from(0x100020)));
    assert!(matches!(captured.state.kind(), ByteStateKind::Zero));
    assert!(
        partial
            .symbolic
            .nodes
            .iter()
            .all(|node| !matches!(node, Expression::WriteWord(_)))
    );
    assert!(
        partial
            .outcomes
            .iter()
            .all(|outcome| { outcome.kind == evm_abstract_protocol::OutcomeKind::Failure })
    );
    partial.validate().unwrap();
}

#[test]
fn symbolic_entry_rollback_refusal_executes_no_submitted_instruction() {
    let input = sparse_mstore_request();
    let code = sparse_mstore_code();
    // Keep the original symbolic recipient as an independent entry-boundary
    // control. Its optional RIPEMD touch requires a five-node rollback MAY root,
    // so this three-node policy refuses before any submitted instruction runs.
    let mut bounded = input.clone();
    bounded.limits.max_memory_bytes = 1_048_576;
    bounded.limits.max_expression_nodes = 3;
    let AnalysisInput::Bytecode(source) = &bounded.input else {
        panic!("expected the original symbolic bytecode input")
    };
    assert!(source.address.is_none());
    assert_eq!(source.bytecode, code);
    let limits = bounded.limits.clone();
    let partial = analyze(bounded).unwrap();
    assert_eq!(partial.metadata.limits, limits);
    assert_eq!(
        partial.metadata.environment.to,
        AddressValue::Symbolic(InputSymbol {
            kind: InputSymbolKind::To,
            index: None,
        })
    );
    assert_eq!(partial.status, AnalysisStatus::Incomplete);
    assert!(!partial.ssa.complete);
    let [frontier] = partial.frontiers.as_slice() else {
        panic!(
            "expected only the symbolic entry refusal: {:?}",
            partial.frontiers
        )
    };
    assert_eq!(
        frontier.reason,
        FrontierDetails::Relations(evm_abstract_protocol::QueryBoundary::ExpressionLimit)
    );
    assert_eq!(frontier.kind, FrontierKind::Relations);
    assert_eq!((frontier.from, frontier.pc), (Some(0), Some(0)));
    assert!(frontier.range.is_none());
    assert!(frontier.payload.is_none());
    let [state] = partial.states.as_slice() else {
        panic!("expected only the symbolic root state")
    };
    let [cfg] = partial.cfg.as_slice() else {
        panic!("expected only the symbolic root CFG block")
    };
    let [block] = partial.ssa.blocks.as_slice() else {
        panic!("expected only the symbolic root SSA block")
    };
    assert_eq!((state.state, cfg.id, block.state), (0, 0, 0));
    assert!(cfg.executed_pcs.is_empty());
    assert!(cfg.entry_stack.is_empty());
    assert!(cfg.exit_stack.is_empty());
    assert_eq!(block.coverage, BlockCoverage::Current);
    assert!(!block.incoming_complete);
    assert!(block.instructions.is_empty());
    assert!(block.phis.is_empty());
    assert!(block.effect_inputs.is_empty());
    assert!(block.open_incoming.is_empty());
    assert_eq!(block.exit_frames, [Vec::<usize>::new()]);
    assert_eq!(block.exit_effect, block.effect);
    assert_eq!(partial.ssa.value_count, 0);
    assert!(partial.ssa.values.is_empty());
    assert!(partial.edges.is_empty());
    assert!(partial.ssa.transitions.is_empty());
    assert!(partial.ssa.deferred_edges.is_empty());
    assert_eq!(state.exit.as_ref(), Some(&state.entry));
    assert_eq!(partial.stores[state.entry.store].touched_default, None);
    let [frame] = state.entry.frames.as_slice() else {
        panic!("expected the original symbolic root frame")
    };
    assert!(frame.stack.is_empty());
    assert_eq!(
        frame.address_value.as_ref(),
        Some(&partial.metadata.environment.to)
    );
    assert_eq!(
        partial.stores[frame.rollback_store].touched_default,
        Some(false)
    );
    let memory = &partial.byte_arrays[frame.memory.unwrap()];
    assert!(memory.memory);
    assert!(buffer_matches(&partial, memory, &[]));
    assert!(memory.cells.is_empty());
    assert_eq!(memory.default.constants, Some(vec!["0x0".into()]));
    let Expression::View(view) = &partial.symbolic.nodes[memory.symbolic.node] else {
        panic!("symbolic entry retains its initial memory view")
    };
    assert!(matches!(
        partial.symbolic.nodes[view.state.node],
        Expression::BytesZero
    ));
    let root = partial.metadata.root_program.unwrap();
    assert_eq!(frame.program, Some(root));
    assert_eq!(cfg.program, Some(root));
    let program = &partial.programs[root];
    assert_eq!(program.bytecode, format!("0x{code}"));
    assert_eq!(
        program.code_address, None,
        "supplied code is not a physical account observation"
    );
    assert_eq!(frame.code_hash.as_ref(), Some(&program.code_hash));
    assert_eq!(partial.disassembly, program.blocks);
    let [source] = program.blocks.as_slice() else {
        panic!("expected the complete original source block")
    };
    assert_eq!((source.id, source.start_pc), (0, 0));
    assert_eq!(cfg.instructions, source.instructions);
    assert_eq!(source.instructions.len(), 20 * 3 + 3);
    assert!(partial.outcomes.iter().all(|outcome| {
        outcome.state == 0 && outcome.kind == evm_abstract_protocol::OutcomeKind::Failure
    }));
    partial.validate().unwrap();
}
