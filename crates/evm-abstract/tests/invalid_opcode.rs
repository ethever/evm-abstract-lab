//! Diagnostics retain execution evidence from the active captured code frame.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        self, Config, DiagnosticKind, ExecutionConfig, FrameCode, InstructionProgress,
        InvalidOpcodeDiagnostic, InvalidOpcodeReason, OutcomeKind, Status,
    },
    bytecode::Program,
    world::{Account, Code, Entry, World},
};
use std::collections::BTreeSet;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn instruction_detail(kind: &DiagnosticKind) -> (u8, Option<&str>, InvalidOpcodeReason) {
    match kind {
        DiagnosticKind::InvalidOpcode(InvalidOpcodeDiagnostic::Instruction {
            opcode,
            mnemonic,
            reason,
        }) => (*opcode, mnemonic.as_deref(), *reason),
        other => panic!("expected ordinary instruction evidence, got {other:?}"),
    }
}

#[test]
fn invalid_bytes_and_fork_rules_have_distinct_native_payloads() {
    for (hex, fork, pc, byte, mnemonic, reason) in [
        (
            "fe",
            Fork::Osaka,
            0,
            0xfe,
            Some("INVALID"),
            InvalidOpcodeReason::ExplicitInvalid,
        ),
        (
            "ef",
            Fork::Osaka,
            0,
            0xef,
            None,
            InvalidOpcodeReason::UndefinedOpcode,
        ),
        (
            "5f1e00",
            Fork::Cancun,
            1,
            0x1e,
            Some("CLZ"),
            InvalidOpcodeReason::UnavailableInFork(Fork::Cancun),
        ),
    ] {
        let view = analysis::analyze(
            Program::from_hex_with_fork(hex, fork).unwrap(),
            Config::default(),
        )
        .unwrap();
        assert_eq!(view.status(), Status::Converged);
        assert!(view.frontiers().is_empty());
        assert_eq!(view.diagnostics().len(), 1);
        let diagnostic = &view.diagnostics()[0];
        assert_eq!(diagnostic.pc, pc);
        assert_eq!(
            instruction_detail(&diagnostic.kind),
            (byte, mnemonic, reason)
        );
        let native = &view.execution().diagnostics()[0];
        assert_eq!(diagnostic.kind, native.kind);
        let captured = view.execution().states()[native.state]
            .entry
            .active()
            .program
            .as_ref()
            .unwrap();
        let instruction = captured
            .blocks()
            .iter()
            .flat_map(|b| &b.instructions)
            .find(|i| i.pc == pc)
            .unwrap();
        assert_eq!(instruction.opcode, byte);
        assert_eq!(instruction.invalid_reason(), Some(reason));
        assert!(!instruction.is_valid());
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["schema_version"], 5);
        assert_eq!(
            json["diagnostics"][0]["kind"]["InvalidOpcode"]["Instruction"]["opcode"],
            byte
        );
        assert_eq!(
            json["diagnostics"][0]["kind"]["InvalidOpcode"]["Instruction"]["mnemonic"].as_str(),
            mnemonic
        );
        assert!(
            view.execution()
                .outcomes()
                .iter()
                .all(|o| o.kind == OutcomeKind::Failure)
        );
    }
    let view = analysis::analyze(
        Program::from_hex_with_fork("5f1e00", Fork::Osaka).unwrap(),
        Config::default(),
    )
    .unwrap();
    assert_eq!(view.status(), Status::Converged);
    assert!(view.diagnostics().is_empty());
    assert_eq!(view.states()[0].executed_pcs, [0, 1, 2]);
}

#[test]
fn future_defined_instructions_retain_their_mnemonic_and_fork_reason() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for (byte, mnemonic) in [
            (0x4b, "SLOTNUM"),
            (0xe6, "DUPN"),
            (0xe7, "SWAPN"),
            (0xe8, "EXCHANGE"),
        ] {
            let view = analysis::analyze(
                Program::decode_with_fork(&[byte], fork).unwrap(),
                Config::default(),
            )
            .unwrap();
            assert_eq!(view.status(), Status::Converged);
            assert_eq!(view.diagnostics().len(), 1);
            assert_eq!(
                instruction_detail(&view.diagnostics()[0].kind),
                (
                    byte,
                    Some(mnemonic),
                    InvalidOpcodeReason::UnavailableInFork(fork)
                )
            );
        }
    }
}

#[test]
fn unreachable_invalid_bytes_are_disassembled_without_execution_diagnostics() {
    let view = analysis::analyze(Program::from_hex("00feef").unwrap(), Config::default()).unwrap();
    assert_eq!(view.program().blocks().len(), 3);
    assert_eq!(view.program().blocks()[1].instructions[0].opcode, 0xfe);
    assert_eq!(view.program().blocks()[2].instructions[0].opcode, 0xef);
    assert_eq!(view.states().len(), 1);
    assert_eq!(view.states()[0].executed_pcs, [0]);
    assert!(view.diagnostics().is_empty());
}

#[test]
fn one_invalid_instruction_retains_each_abstract_state_identity() {
    // An unknown branch reaches the same INVALID with stack heights zero and one.
    let view = analysis::analyze(
        Program::from_hex("5f35600a576001600a565bfe").unwrap(),
        Config {
            context_depth: 0,
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(view.status(), Status::Converged);
    assert_eq!(view.diagnostics().len(), 2);
    assert!(view.diagnostics().iter().all(|d| d.pc == 11));
    assert_eq!(
        view.diagnostics()
            .iter()
            .map(|d| d.state)
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
    let heights = view
        .diagnostics()
        .iter()
        .map(|d| {
            assert_eq!(instruction_detail(&d.kind).0, 0xfe);
            view.states()[d.state].key.stack_height
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(heights, BTreeSet::from([0, 1]));
}

#[test]
fn callees_at_the_same_pc_use_their_own_captured_programs() {
    let caller = "5f5f5f5f5f61020061fffff1505f5f5f5f5f61030061fffff100";
    for use_summaries in [false, true] {
        let mut world = World::new(Fork::Osaka, "invalid callee provenance");
        for (owner, code) in [(0x101, caller), (0x200, "fe"), (0x300, "ef")] {
            world
                .insert(
                    address(owner),
                    Account::from_hex(code, world.fork()).unwrap(),
                )
                .unwrap();
        }
        let report = analysis::analyze_world(
            world,
            Entry::new(address(0x101)),
            ExecutionConfig {
                use_summaries,
                ..ExecutionConfig::default()
            },
        )
        .unwrap();
        assert_eq!(report.status(), Status::Converged);
        let mut seen = BTreeSet::new();
        for diagnostic in report.diagnostics() {
            let (opcode, _, _) = instruction_detail(&diagnostic.kind);
            assert_eq!(diagnostic.pc, 0);
            let state = &report.states()[diagnostic.state];
            assert_eq!(state.key.frames.len(), 2);
            let frame = state.entry.active();
            assert_eq!(frame.code, FrameCode::Runtime);
            assert_eq!(frame.program.as_ref().unwrap().bytes()[0], opcode);
            assert_eq!(
                state.execution_evidence().unwrap().instructions(),
                [InstructionProgress::Faulted]
            );
            seen.insert((frame.key.code_address, opcode));
        }
        assert_eq!(
            seen,
            BTreeSet::from([(address(0x200), 0xfe), (address(0x300), 0xef)])
        );
    }
}

#[test]
fn initcode_fault_uses_initcode_byte_at_pc_zero() {
    let creator = address(0x101);
    let destination = creator.create(0);
    let mut world = World::new(Fork::Osaka, "invalid initcode provenance");
    // Put 0xfe into memory[0], then CREATE with this one-byte initcode.
    world
        .insert(
            creator,
            Account::from_hex("60fe5f5360015f5ff000", world.fork()).unwrap(),
        )
        .unwrap();
    world.insert(destination, Account::absent()).unwrap();
    let report =
        analysis::analyze_world(world, Entry::new(creator), ExecutionConfig::default()).unwrap();
    assert_eq!(report.status(), Status::Converged);
    assert_eq!(report.diagnostics().len(), 1);
    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.pc, 0);
    assert_eq!(
        instruction_detail(&diagnostic.kind),
        (0xfe, Some("INVALID"), InvalidOpcodeReason::ExplicitInvalid)
    );
    let state = &report.states()[diagnostic.state];
    let frame = state.entry.active();
    assert_eq!(frame.code, FrameCode::InitCode);
    assert_eq!(frame.key.code_address, destination);
    assert_eq!(frame.program.as_ref().unwrap().bytes(), [0xfe]);
    assert_eq!(
        state.execution_evidence().unwrap().instructions(),
        [InstructionProgress::Faulted]
    );
}

#[test]
fn nested_delegation_entry_has_a_verified_byte_and_no_instruction_receipt() {
    let mut world = World::new(Fork::Osaka, "nested delegation evidence");
    for (owner, implementation) in [(0x101, 0x200), (0x200, 0x300)] {
        world
            .insert(
                address(owner),
                Account {
                    code: Code::Delegation(address(implementation)),
                    ..Account::empty()
                },
            )
            .unwrap();
    }
    let report = analysis::analyze_world(
        world,
        Entry::new(address(0x101)),
        ExecutionConfig::default(),
    )
    .unwrap();
    assert_eq!(report.status(), Status::Converged);
    assert_eq!(report.diagnostics().len(), 1);
    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.pc, 0);
    assert_eq!(
        diagnostic.kind,
        DiagnosticKind::InvalidOpcode(InvalidOpcodeDiagnostic::DelegationEntry {
            opcode: Some(0xef),
            detail: "nested delegation indicator cannot be executed".to_owned(),
            byte_unavailable: None,
        })
    );
    let state = &report.states()[diagnostic.state];
    assert_eq!(state.entry.active().key.code_address, address(0x200));
    assert_eq!(state.entry.active().code, FrameCode::InvalidDelegation);
    assert!(state.entry.active().program.is_none());
    assert!(state.executed_pcs.is_empty());
    assert!(
        state
            .execution_evidence()
            .unwrap()
            .instructions()
            .is_empty()
    );
    assert!(
        report
            .outcomes()
            .iter()
            .all(|o| o.kind == OutcomeKind::Failure)
    );
}
