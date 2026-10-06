//! Osaka 新指令必须影响真实 CFG 和 SSA，不能只替换文档中的 fork 名。

use evm_abstract::{
    Fork, U256,
    analysis::{self, Config, DiagnosticKind},
    bytecode::{DecodeError, Program},
    domain::{Domain, Value, provenance::Origin},
    ssa,
};
use revm_bytecode::eip7702::Eip7702DecodeError;
use std::num::NonZeroUsize;

#[test]
fn default_mainnet_recovers_a_clz_computed_jump() {
    // CLZ(1)=255；PUSH 247、SWAP1、SUB 后得到 8，合法目标在 pc=8。
    let program = Program::from_hex("60011e60f79003565b602a00").unwrap();
    let analysis = analysis::analyze(program, Config::default()).unwrap();
    assert_eq!(analysis.edges().len(), 1);
    let target = &analysis.states()[analysis.edges()[0].to];
    assert_eq!(
        analysis.program().blocks()[target.key.basic_block_index].start_pc,
        8
    );
    assert!(!analysis.diagnostics().iter().any(|d| matches!(
        d.kind,
        DiagnosticKind::UnknownJump | DiagnosticKind::InvalidOpcode
    )));
    assert!(target.exit_stack[0].contains(U256::from(42)));
    let ir = ssa::build(&analysis).unwrap();
    assert!(
        ir.blocks()[0]
            .instructions
            .iter()
            .any(|i| i.opcode == 0x1e && !i.fault && i.results.len() == 1)
    );
}

#[test]
fn clz_zero_folds_to_256() {
    let result = Domain::default().apply(0x1e, &[Value::constant(U256::ZERO)]);
    assert_eq!(result.singleton(), Some(U256::from(256)));
    assert_eq!(
        result.constants(),
        Value::constant(U256::from(256)).constants()
    );
    assert!(
        result
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Arithmetic)
    );
}

#[test]
fn delegation_designator_is_not_an_executable_runtime() {
    let raw_code = "ef01001111111111111111111111111111111111111111";
    for fork in [Fork::Prague, Fork::Osaka] {
        let error = Program::from_hex_with_fork(raw_code, fork).unwrap_err();
        assert!(
            matches!(error, DecodeError::DelegatedCode { address } if address.as_slice()==[0x11;20])
        );
    }
    let old = analysis::analyze(
        Program::from_hex_with_fork(raw_code, Fork::Cancun).unwrap(),
        Config::default(),
    )
    .unwrap();
    assert!(
        old.diagnostics()
            .iter()
            .any(|d| d.pc == 0 && d.kind == DiagnosticKind::InvalidOpcode)
    );
}

#[test]
fn clz_is_disabled_before_osaka_and_is_a_fault_in_ssa() {
    for fork in [Fork::Cancun, Fork::Prague] {
        let program = Program::from_hex_with_fork("60011e60f79003565b602a00", fork).unwrap();
        assert_eq!(program.fork(), fork);
        let analysis = analysis::analyze(program, Config::default()).unwrap();
        assert!(analysis.edges().is_empty());
        assert_eq!(analysis.states()[0].executed_pcs, [0, 2]);
        assert!(
            analysis
                .diagnostics()
                .iter()
                .any(|d| d.pc == 2 && d.kind == DiagnosticKind::InvalidOpcode)
        );
        let ir = ssa::build(&analysis).unwrap();
        assert!(ir.blocks()[0].instructions[1].fault);
    }
}

#[test]
fn future_instructions_remain_invalid_on_all_supported_forks() {
    // SLOTNUM/DUPN/SWAPN/EXCHANGE 属于 Amsterdam；其立即数不能提前吞掉
    // Osaka/Cancun 下真正的 JUMPDEST。
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for op in [0x4b, 0xe6, 0xe7, 0xe8] {
            let program = Program::decode_with_fork(&[op, 0x5b, 0], fork).unwrap();
            assert!(program.jumpdest_blocks().contains_key(&1));
            let analysis = analysis::analyze(program, Config::default()).unwrap();
            assert_eq!(analysis.states()[0].executed_pcs, [0]);
            assert!(
                analysis
                    .diagnostics()
                    .iter()
                    .any(|d| d.kind == DiagnosticKind::InvalidOpcode)
            );
        }
    }
}

#[test]
fn fork_selection_is_explicit_and_serialized() {
    assert_eq!(Fork::default(), Fork::Osaka);
    assert_eq!(
        Fork::Osaka.spec_id(),
        revm_bytecode::primitives::hardfork::SpecId::OSAKA
    );
    for (name, fork) in [
        ("cancun", Fork::Cancun),
        ("prague", Fork::Prague),
        ("osaka", Fork::Osaka),
    ] {
        assert_eq!(name.parse::<Fork>().unwrap(), fork);
        assert_eq!(fork.to_string(), name);
        let program = Program::from_hex_with_fork("00", fork).unwrap();
        assert_eq!(serde_json::to_value(&program).unwrap()["fork"], name);
    }
    assert!("amsterdam".parse::<Fork>().is_err());
}

#[test]
fn clz_of_sets_and_product_top_keeps_the_full_output_range() {
    let domain = Domain::default();
    let inputs = domain.join(
        &Value::constant(U256::ZERO),
        &Value::constant(U256::from(1)),
    );
    let result = domain.apply(0x1e, &[inputs]);
    assert_eq!(result.constants().unwrap().len(), 2);
    assert!(result.contains(U256::from(255)) && result.contains(U256::from(256)));
    let open = domain.apply(0x1e, &[Value::top()]);
    assert!(open.constants().is_none());
    assert_eq!(
        open.interval().unsigned_bounds(),
        (U256::ZERO, U256::from(256))
    );
    for value in 0_u64..=256 {
        assert!(open.contains(U256::from(value)));
    }
    assert!(!open.contains(U256::from(257)));
}

#[test]
fn constants_only_clz_requires_capacity_for_the_complete_range() {
    let bounded = Domain::new(NonZeroUsize::new(8).unwrap());
    assert_eq!(bounded.apply(0x1e, &[Value::top()]), Value::top());
    let full = Domain::new(NonZeroUsize::new(257).unwrap()).apply(0x1e, &[Value::top()]);
    assert_eq!(full.constants().unwrap().len(), 257);
    for value in 0_u64..=256 {
        assert!(full.constants().unwrap().contains(&U256::from(value)));
    }
    assert!(!full.contains(U256::from(257)));
}

#[test]
fn malformed_delegation_preserves_revm_parse_errors() {
    assert!(matches!(
        Program::from_hex("ef0100"),
        Err(DecodeError::InvalidDelegation(
            Eip7702DecodeError::InvalidLength
        ))
    ));
    let future = "ef01011111111111111111111111111111111111111111";
    assert!(matches!(
        Program::from_hex(future),
        Err(DecodeError::InvalidDelegation(
            Eip7702DecodeError::UnsupportedVersion
        ))
    ));
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        assert!(matches!(
            Program::from_hex_with_fork("ef0001", fork),
            Err(DecodeError::UnsupportedEof)
        ));
    }
}
