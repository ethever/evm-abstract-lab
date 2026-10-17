use super::{
    FactScope, FactSource, NormalizationInvariantKind, NormalizedOperation, ValueOperand,
    ValueTransformation, build_normalized_partial_world, build_normalized_world,
};
use crate::{
    Address, Fork, U256,
    analysis::{self, ExecutionConfig, FrontierReason, InstructionProgress, Status, WorldAnalysis},
    domain::AbstractValue,
    ssa::{PartialBlockCoverage, SsaError},
    world::{Account, ByteArray, Entry, EvmEnvironment, World},
};
use revm_bytecode::opcode;

fn config() -> ExecutionConfig {
    let mut config = ExecutionConfig::default();
    config.analysis.context_depth = 0;
    config.analysis.relations.enabled = false;
    config.use_summaries = false;
    config
}

fn run(code: &str, config: ExecutionConfig, value: AbstractValue) -> WorldAnalysis {
    run_with_calldata(code, config, value, ByteArray::empty())
}

fn run_with_calldata(
    code: &str,
    config: ExecutionConfig,
    value: AbstractValue,
    calldata: ByteArray,
) -> WorldAnalysis {
    let root = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "normalized SSA value semantics");
    world
        .insert(root, Account::from_hex(code, Fork::Osaka).unwrap())
        .unwrap();
    analysis::analyze_world(
        world,
        Entry {
            address: root,
            environment: EvmEnvironment {
                to: root.into(),
                caller: Address::repeat_byte(0x33).into(),
                value,
                calldata,
                ..EvmEnvironment::default()
            },
        },
        config,
    )
    .unwrap()
}

fn pending_mload(code: &str, pc: usize, value: AbstractValue) -> WorldAnalysis {
    let mut config = config();
    config.max_memory_bytes = 32;
    let complete = run(code, config.clone(), value.clone());
    assert_eq!(complete.status(), Status::Converged);
    assert!(complete.states().iter().any(|state| {
        state
            .executed_pcs
            .iter()
            .position(|at| *at == pc)
            .is_some_and(|index| {
                state.execution_evidence().unwrap().instructions()[index]
                    == InstructionProgress::Completed
            })
    }));
    // Both fixtures are acyclic. Find the first budget admitting MLOAD's
    // visit, including all earlier transfers and root admission. The next
    // positive read-scan charge then refuses after consuming its offset.
    let (mut minimum, mut maximum) = (1, complete.work());
    while minimum < maximum {
        let midpoint = minimum + (maximum - minimum) / 2;
        config.max_work = midpoint;
        let candidate = run(code, config.clone(), value.clone());
        if candidate
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&pc))
        {
            maximum = midpoint;
        } else {
            minimum = midpoint + 1;
        }
    }
    config.max_work = minimum;
    let analysis = run(code, config, value);
    assert_eq!(analysis.status(), Status::Incomplete);
    let state = analysis
        .states()
        .iter()
        .find(|state| state.executed_pcs.contains(&pc))
        .unwrap();
    assert_eq!(state.executed_pcs.last(), Some(&pc));
    assert_eq!(
        state.execution_evidence().unwrap().instructions().last(),
        Some(&InstructionProgress::OperandsConsumed)
    );
    let [frontier] = analysis.frontiers() else {
        panic!("expected exactly the refused MLOAD read scan")
    };
    assert_eq!(frontier.reason, FrontierReason::Work);
    assert_eq!((frontier.from, frontier.pc), (Some(state.id), Some(pc)));
    analysis
}

#[test]
fn folding_propagates_constants_but_keeps_every_original_definition_and_effect() {
    let analysis = run("600260030160040200", config(), AbstractValue::top());
    let raw = crate::ssa::build_world(&analysis).unwrap();
    let ir = build_normalized_world(&analysis).unwrap();
    assert_eq!(ir.value_count(), raw.value_count());
    assert_eq!(ir.effect_count(), raw.effect_count());
    assert_eq!(
        ir.blocks()[0].instructions.len(),
        raw.blocks()[0].instructions.len()
    );
    assert_eq!(
        ir.blocks()[0].instructions[2].operation,
        NormalizedOperation::Constant {
            value: U256::from(5)
        }
    );
    assert_eq!(
        ir.blocks()[0].instructions[4].operation,
        NormalizedOperation::Constant {
            value: U256::from(20)
        }
    );
    assert_eq!(ir.blocks()[0].instructions[2].origin.opcode, opcode::ADD);
    assert_eq!(ir.blocks()[0].instructions[4].origin.opcode, opcode::MUL);
    let product = ir.blocks()[0].instructions[4].origin.results[0];
    assert_eq!(
        ir.mappings()[product].transformation,
        ValueTransformation::ConstantFold
    );
    assert_eq!(
        ir.mappings()[product].dependencies,
        ir.blocks()[0].instructions[4].origin.operands
    );
    ir.verify(&analysis).unwrap();
}

#[test]
fn copies_and_identity_results_retain_unique_definitions_without_cse_or_dce() {
    let analysis = run("345f0180811800", config(), AbstractValue::top());
    let ir = build_normalized_world(&analysis).unwrap();
    let rows = &ir.blocks()[0].instructions;
    let input = rows[0].origin.results[0];
    let copy = rows[2].origin.results[0];
    assert_ne!(input, copy);
    assert_eq!(
        rows[2].operation,
        NormalizedOperation::Copy {
            value: ValueOperand::Value(input)
        }
    );
    assert_eq!(ir.mappings()[copy].equivalent, Some(input));
    assert_eq!(ir.mappings()[copy].dependencies, rows[2].origin.operands);
    assert_eq!(
        rows[5].operation,
        NormalizedOperation::Constant { value: U256::ZERO }
    );
    assert!(rows[3].origin.results.is_empty());
    assert!(rows[4].origin.results.is_empty());
    assert_eq!(rows.len(), 7);
}

#[test]
fn complete_snapshot_results_fold_environment_reads_with_definition_scopes() {
    let analysis = run(
        "3460030100",
        config(),
        AbstractValue::constant(U256::from(7)),
    );
    let ir = build_normalized_world(&analysis).unwrap();
    assert_eq!(
        ir.blocks()[0].instructions[0].operation,
        NormalizedOperation::Constant {
            value: U256::from(7)
        }
    );
    assert_eq!(
        ir.blocks()[0].instructions[2].operation,
        NormalizedOperation::Constant {
            value: U256::from(10)
        }
    );
    let input = ir.blocks()[0].instructions[0].origin.results[0];
    assert!(ir.facts().iter().any(|fact| fact.value == input
        && fact.scope == FactScope::Definition
        && fact.source == FactSource::Instruction
        && fact.constant == Some(U256::from(7))));
    assert_eq!(
        ir.mappings()[input].transformation,
        ValueTransformation::AnalyzedConstant
    );
}

#[test]
fn partial_snapshots_do_not_certify_definition_rewrites_but_literals_still_fold() {
    let analysis = pending_mload(
        "34600301600260030160405100",
        11,
        AbstractValue::constant(U256::from(7)),
    );
    assert_eq!(analysis.status(), Status::Incomplete);
    let ir = build_normalized_partial_world(&analysis).unwrap();
    assert!(!ir.complete());
    let rows = &ir.blocks()[0].instructions;
    assert!(matches!(
        rows[0].operation,
        NormalizedOperation::Opcode {
            opcode: opcode::CALLVALUE,
            ..
        }
    ));
    assert!(matches!(
        rows[2].operation,
        NormalizedOperation::Opcode {
            opcode: opcode::ADD,
            ..
        }
    ));
    assert_eq!(
        rows[5].operation,
        NormalizedOperation::Constant {
            value: U256::from(5)
        }
    );
    let input = rows[0].origin.results[0];
    assert!(ir.facts().iter().any(|fact| fact.value == input
        && fact.scope == FactScope::State { state: 0 }
        && fact.constant == Some(U256::from(7))));
    assert!(
        ir.facts()
            .iter()
            .filter(|fact| fact.value == input)
            .all(|fact| { fact.scope != FactScope::Definition })
    );
    let pending = rows.last().unwrap();
    assert_eq!(pending.origin.pc, 11);
    assert_eq!(pending.progress, InstructionProgress::OperandsConsumed);
    assert!(pending.origin.results.is_empty());
    assert!(matches!(
        pending.operation,
        NormalizedOperation::Opcode {
            opcode: opcode::MLOAD,
            ..
        }
    ));
    ir.verify(&analysis).unwrap();
}

#[test]
fn verifier_rejects_origin_rewrites_and_unproved_constants_scopes_and_aliases() {
    let analysis = run("345f0100", config(), AbstractValue::top());
    let valid = build_normalized_world(&analysis).unwrap();
    let kind = |ir: &super::NormalizedSsa| {
        let SsaError::Normalization(error) = ir.verify(&analysis).unwrap_err() else {
            panic!("expected normalization invariant")
        };
        error.kind
    };
    let mut wrong = valid.clone();
    wrong.blocks[0].instructions[2].origin.opcode = opcode::MUL;
    assert_eq!(kind(&wrong), NormalizationInvariantKind::Origin);
    let mut wrong = valid.clone();
    wrong.blocks[0].instructions[2].operation = NormalizedOperation::Constant {
        value: U256::from(123),
    };
    assert_eq!(kind(&wrong), NormalizationInvariantKind::Operation);
    let mut wrong = valid.clone();
    wrong.facts[0].constant = Some(U256::from(123));
    assert_eq!(kind(&wrong), NormalizationInvariantKind::Facts);
    let mut wrong = valid.clone();
    wrong.mappings[2].equivalent = Some(2);
    assert_eq!(kind(&wrong), NormalizationInvariantKind::Mappings);
}

#[test]
fn stale_and_unexecuted_coverage_keep_their_parameters_without_a_body() {
    let mut limited = config();
    limited.analysis.max_transfers = 2;
    let analysis = run("5f5b600101600156", limited, AbstractValue::top());
    let ir = build_normalized_partial_world(&analysis).unwrap();
    let stale = ir
        .blocks()
        .iter()
        .find(|block| block.coverage == PartialBlockCoverage::Stale)
        .unwrap();
    assert!(stale.instructions.is_empty());
    assert!(
        ir.deferred_edges()
            .iter()
            .any(|edge| edge.from == stale.state)
    );
    assert_eq!(ir.frontiers().len(), analysis.frontiers().len());
    assert!(!stale.incoming_complete);
}

#[test]
fn exact_folding_uses_evm_wrap_signed_zero_modulus_and_shift_semantics() {
    let min_signed = U256::from(1) << 255usize;
    let negative = |value| U256::ZERO.wrapping_sub(U256::from(value));
    let cases = [
        (opcode::ADD, vec![U256::MAX, U256::from(1)], U256::ZERO),
        (opcode::SUB, vec![U256::ZERO, U256::from(1)], U256::MAX),
        (opcode::DIV, vec![U256::from(7), U256::ZERO], U256::ZERO),
        (opcode::SDIV, vec![min_signed, U256::MAX], min_signed),
        (
            opcode::SDIV,
            vec![negative(7u64), U256::from(3)],
            negative(2u64),
        ),
        (opcode::SMOD, vec![negative(7u64), U256::from(3)], U256::MAX),
        (
            opcode::ADDMOD,
            vec![U256::MAX, U256::from(2), U256::from(5)],
            U256::from(2),
        ),
        (
            opcode::MULMOD,
            vec![U256::MAX, U256::from(2), U256::from(7)],
            U256::from(2),
        ),
        (
            opcode::ADDMOD,
            vec![U256::MAX, U256::MAX, U256::ZERO],
            U256::ZERO,
        ),
        (opcode::EXP, vec![U256::ZERO, U256::ZERO], U256::from(1)),
        (
            opcode::SIGNEXTEND,
            vec![U256::ZERO, U256::from(0x80)],
            negative(128u64),
        ),
        (
            opcode::SHL,
            vec![U256::from(256), U256::from(1)],
            U256::ZERO,
        ),
        (opcode::SHR, vec![U256::from(256), U256::MAX], U256::ZERO),
        (opcode::SAR, vec![U256::from(256), min_signed], U256::MAX),
        (opcode::BYTE, vec![U256::from(32), U256::MAX], U256::ZERO),
    ];
    for (op, operands, result) in cases {
        let mut code = operands
            .iter()
            .rev()
            .map(|value| format!("7f{value:064x}"))
            .collect::<String>();
        code.push_str(&format!("{op:02x}00"));
        let analysis = run(&code, config(), AbstractValue::top());
        let ir = build_normalized_world(&analysis).unwrap();
        let operation = ir.blocks()[0]
            .instructions
            .iter()
            .find(|step| step.origin.opcode == op && step.origin.immediate.is_none())
            .unwrap();
        assert_eq!(
            operation.operation,
            NormalizedOperation::Constant { value: result },
            "opcode={op:02x}, operands={operands:?}"
        );
        assert_eq!(
            ir.mappings()[operation.origin.results[0]]
                .dependencies
                .len(),
            operands.len()
        );
    }
}

#[test]
fn zero_division_and_zero_to_unknown_power_do_not_gain_unsafe_identities() {
    for (code, op) in [("34800400", opcode::DIV), ("345f0a00", opcode::EXP)] {
        let analysis = run(code, config(), AbstractValue::top());
        let ir = build_normalized_world(&analysis).unwrap();
        let operation = ir.blocks()[0]
            .instructions
            .iter()
            .find(|step| step.origin.opcode == op)
            .unwrap();
        assert!(
            matches!(operation.operation, NormalizedOperation::Opcode { .. }),
            "opcode={op:02x}"
        );
    }
}

#[test]
fn different_values_with_the_same_source_and_abstract_summary_are_not_aliases() {
    let analysis = run_with_calldata(
        "5f356020351800",
        config(),
        AbstractValue::top(),
        ByteArray::unknown(),
    );
    let ir = build_normalized_world(&analysis).unwrap();
    let xor = ir.blocks()[0]
        .instructions
        .iter()
        .find(|step| step.origin.opcode == opcode::XOR)
        .unwrap();
    assert_ne!(xor.origin.operands[0], xor.origin.operands[1]);
    assert!(matches!(
        xor.operation,
        NormalizedOperation::Opcode {
            opcode: opcode::XOR,
            ..
        }
    ));
    assert!(ir.mappings()[xor.origin.results[0]].equivalent.is_none());
}

#[test]
fn closed_join_facts_propagate_to_uses_without_replacing_or_simplifying_the_phi() {
    let analysis = run(
        "34600a576007600d56005b60075b60030100",
        config(),
        AbstractValue::top(),
    );
    let raw = crate::ssa::build_world(&analysis).unwrap();
    let ir = build_normalized_world(&analysis).unwrap();
    let merge = ir
        .blocks()
        .iter()
        .find(|block| block.phis.first().is_some_and(|phi| phi.inputs.len() == 2))
        .unwrap();
    assert_eq!(merge.phis, raw.blocks()[merge.state].phis);
    let phi = &merge.phis[0];
    assert!(ir.mappings()[phi.result].equivalent.is_none());
    assert_eq!(
        ir.mappings()[phi.result].transformation,
        ValueTransformation::Original
    );
    assert!(ir.facts().iter().any(|fact| fact.value == phi.result
        && fact.scope == FactScope::Definition
        && fact.constant == Some(U256::from(7))));
    let add = merge
        .instructions
        .iter()
        .find(|step| step.origin.opcode == opcode::ADD)
        .unwrap();
    assert_eq!(
        add.operation,
        NormalizedOperation::Constant {
            value: U256::from(10)
        }
    );
    assert!(
        ir.mappings()[add.origin.results[0]]
            .dependencies
            .contains(&phi.result)
    );
}

#[test]
fn open_phi_observation_cannot_fold_its_consumer_but_self_identity_can() {
    let analysis = pending_mload(
        "34600556005b600301801860405100",
        13,
        AbstractValue::constant(U256::from(7)),
    );
    assert_eq!(analysis.status(), Status::Incomplete);
    let ir = build_normalized_partial_world(&analysis).unwrap();
    let body = ir
        .blocks()
        .iter()
        .find(|block| !block.phis.is_empty() && block.coverage == PartialBlockCoverage::Current)
        .unwrap();
    assert!(!body.incoming_complete);
    let phi = &body.phis[0];
    assert_eq!(phi.inputs.len(), 1);
    assert!(ir.facts().iter().any(|fact| fact.value == phi.result
        && fact.scope == FactScope::State { state: body.state }
        && fact.constant == Some(U256::from(7))));
    assert!(
        ir.facts()
            .iter()
            .filter(|fact| fact.value == phi.result)
            .all(|fact| fact.scope != FactScope::Definition)
    );
    let add = body
        .instructions
        .iter()
        .find(|step| step.origin.opcode == opcode::ADD)
        .unwrap();
    assert_eq!(
        add.operation,
        NormalizedOperation::Opcode {
            opcode: opcode::ADD,
            operands: vec![
                ValueOperand::Constant(U256::from(3)),
                ValueOperand::Value(phi.result)
            ]
        }
    );
    let xor = body
        .instructions
        .iter()
        .find(|step| step.origin.opcode == opcode::XOR)
        .unwrap();
    assert_eq!(
        xor.operation,
        NormalizedOperation::Constant { value: U256::ZERO }
    );
    assert!(
        ir.facts()
            .iter()
            .any(|fact| fact.value == xor.origin.results[0]
                && fact.scope == FactScope::Definition
                && fact.source == FactSource::AlgebraicIdentity)
    );
    let pending = body.instructions.last().unwrap();
    assert_eq!(
        (pending.origin.pc, pending.origin.opcode),
        (13, opcode::MLOAD)
    );
    assert_eq!(pending.progress, InstructionProgress::OperandsConsumed);
    assert!(pending.origin.results.is_empty());
    ir.verify(&analysis).unwrap();
    // Tampering with the observed scope cannot supply the missing proof.
    let mut wrong = ir.clone();
    let observation = wrong
        .facts
        .iter_mut()
        .find(|fact| fact.value == phi.result)
        .unwrap();
    observation.scope = FactScope::Definition;
    assert!(
        matches!(wrong.verify(&analysis), Err(SsaError::Normalization(error)) if error.kind == NormalizationInvariantKind::Facts)
    );
}

#[test]
fn singleton_from_an_interval_folds_even_when_the_finite_component_is_top() {
    let input = AbstractValue::unsigned_range(U256::from(7), U256::from(7)).unwrap();
    assert!(input.constants().is_none());
    assert_eq!(input.singleton(), Some(U256::from(7)));
    let analysis = run("3460030100", config(), input);
    let ir = build_normalized_world(&analysis).unwrap();
    let value = ir.blocks()[0].instructions[0].origin.results[0];
    assert!(ir.facts().iter().any(|fact| {
        fact.value == value
            && fact
                .numeric
                .as_ref()
                .is_some_and(|numeric| numeric.constants().is_none())
            && fact.constant == Some(U256::from(7))
    }));
    assert_eq!(
        ir.blocks()[0].instructions[2].operation,
        NormalizedOperation::Constant {
            value: U256::from(10)
        }
    );
}

#[test]
fn unknown_call_preserves_every_supported_dispatch_deferred_edge_and_frontier() {
    let root = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "normalized unknown call");
    world
        .insert(
            Address::repeat_byte(0x44),
            Account::from_hex("365f5ff000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
        .insert(
            root,
            Account::from_hex("5f5f5f5f5f345af100", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
        .insert(Address::repeat_byte(0x22), Account::empty())
        .unwrap();
    world
        .insert(Address::from_word(U256::from(4).into()), Account::empty())
        .unwrap();
    let mut options = config();
    options.max_call_depth = 3;
    let analysis = analysis::analyze_world(
        world,
        Entry {
            address: root,
            environment: EvmEnvironment {
                to: root.into(),
                caller: Address::repeat_byte(0x33).into(),
                value: AbstractValue::top(),
                calldata: ByteArray::empty(),
                ..EvmEnvironment::default()
            },
        },
        options,
    )
    .unwrap();
    let raw = crate::ssa::build_partial_world(&analysis).unwrap();
    let ir = build_normalized_partial_world(&analysis).unwrap();
    assert_eq!(ir.status(), Status::Incomplete);
    assert_eq!(ir.transitions(), raw.transitions());
    assert_eq!(ir.deferred_edges(), raw.deferred_edges());
    assert_eq!(ir.frontiers().len(), raw.frontiers().len());
    for call in ir
        .blocks()
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|step| step.origin.opcode == opcode::CALL)
    {
        assert_eq!(call.progress, InstructionProgress::Dispatched);
        assert!(call.origin.results.is_empty());
        assert!(matches!(
            call.operation,
            NormalizedOperation::Opcode {
                opcode: opcode::CALL,
                ..
            }
        ));
    }
    ir.verify(&analysis).unwrap();
}
