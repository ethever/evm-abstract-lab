//! The single-program API must be a view of the native state machine, not a
//! second interpreter with different memory, storage or external-call rules.

use evm_abstract::{
    U256,
    analysis::{self, Config, EdgeKind, FrontierReason, Status},
    bytecode::Program,
    ssa,
};

#[test]
fn single_program_control_flow_uses_native_memory_and_storage_feedback() {
    for code in ["60075f525f51565b00", "60075f555f54565b00"] {
        let view = analysis::analyze(Program::from_hex(code).unwrap(), Config::default()).unwrap();
        assert_eq!(view.status(), Status::Converged);
        assert_eq!(view.edges().len(), 1);
        assert_eq!(view.edges()[0].kind, EdgeKind::Jump);
        assert_eq!(
            view.program().blocks()[view.states()[view.edges()[0].to].key.block].start_pc,
            7
        );
        assert_eq!(view.execution().states().len(), view.states().len());
        ssa::build(&view).unwrap().verify(&view).unwrap();
        ssa::build_world(view.execution())
            .unwrap()
            .verify(view.execution())
            .unwrap();
    }
}

#[test]
fn bytecode_only_inputs_do_not_assume_a_concrete_entry_environment() {
    let view =
        analysis::analyze(Program::from_hex("3032333400").unwrap(), Config::default()).unwrap();
    assert_eq!(view.states()[0].exit_stack.len(), 4);
    assert!(
        view.states()[0]
            .exit_stack
            .iter()
            .all(|value| value.constants().is_none())
    );
    assert!(view.execution().entry().value.contains(U256::MAX));
}

#[test]
fn external_code_omitted_by_single_program_input_remains_incomplete() {
    let view = analysis::analyze(
        Program::from_hex("5f5f5f5f5f61123461fffff100").unwrap(),
        Config::default(),
    )
    .unwrap();
    assert_eq!(view.status(), Status::Incomplete);
    assert!(
        view.execution()
            .frontiers()
            .iter()
            .any(|f| matches!(f.reason, FrontierReason::MissingCode(_)))
    );
    assert!(ssa::build(&view).is_err());
    assert!(ssa::build_world(view.execution()).is_err());
}
