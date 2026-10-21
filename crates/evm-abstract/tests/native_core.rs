//! The single-program API must be a view of the native state machine, not a
//! second interpreter with different memory, storage or external-call rules.
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisInput, EvmAnalysis, ExecutionConfig};

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    U256,
    analysis::{EdgeKind, FrameCode, Status},
    bytecode::Program,
    ssa,
};

#[test]
fn single_program_control_flow_uses_native_memory_and_storage_feedback() {
    for code in ["60075f525f51565b00", "60075f555f54565b00"] {
        let view = native::run(
            &native::product(),
            AnalysisInput::<NumericValue>::from_program(
                Program::from_hex(code).unwrap(),
                Default::default(),
            )
            .unwrap(),
            ExecutionConfig::default(),
            &Control::default(),
        )
        .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
        .unwrap();
        assert_eq!(view.status(), Status::Converged);
        assert_eq!(view.edges().len(), 1);
        assert_eq!(
            view.edges()[0].kind,
            evm_abstract::analysis::MachineEdgeKind::Intraprocedural(EdgeKind::Jump)
        );
        assert_eq!(
            view.program().blocks()[view.states()[view.edges()[0].to].key.basic_block_index]
                .start_pc,
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
    let view = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex("3032333400").unwrap(),
            Default::default(),
        )
        .unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    assert_eq!(view.states()[0].exit_stack.len(), 4);
    assert!(
        view.states()[0]
            .exit_stack
            .iter()
            .all(|value| value.constants().is_none())
    );
    assert!(
        view.execution()
            .entry()
            .environment
            .value
            .contains(U256::MAX)
    );
}

#[test]
fn external_code_omitted_by_single_program_input_uses_native_interaction_summaries() {
    let view = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex("5f5f5f5f5f61123461fffff100").unwrap(),
            evm_abstract::world::EvmEnvironmentValues {
                // Distinguish omitted external code from a symbolic To alias that
                // can instead call the known root recursively.
                to: alloy_primitives::Address::repeat_byte(0x11).into(),
                ..evm_abstract::world::EvmEnvironmentValues::<NumericValue>::default()
            },
        )
        .unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    assert_eq!(view.status(), Status::Converged);
    assert!(
        view.execution()
            .states()
            .iter()
            .any(|state| state.active().mode == FrameCode::Unknown)
    );
    assert!(!view.execution().procedure_summaries().is_empty());
    assert!(
        view.execution()
            .procedure_summaries()
            .iter()
            .all(|summary| summary.closed)
    );
    for projected in view.states() {
        let native_id = view.execution_state_id(projected.id).unwrap();
        let native = &view.execution().states()[native_id];
        assert_eq!(native.entry.active().program.as_ref(), Some(view.program()));
        assert_eq!(projected.entry_stack, native.entry.active().stack);
        assert_eq!(projected.exit_stack, native.exit_stack);
    }
    for edge in view.edges() {
        let from = view.execution_state_id(edge.from).unwrap();
        let to = view.execution_state_id(edge.to).unwrap();
        assert!(
            view.execution().edges().iter().any(|native| {
                native.from == from && native.to == to && native.kind == edge.kind
            })
        );
    }
    ssa::build_world(view.execution())
        .unwrap()
        .verify(view.execution())
        .unwrap();
}

#[path = "support/root_input.rs"]
mod root_input;

#[test]
fn product_and_finite_inputs_share_native_scope_and_root_coverage() {
    root_input::assert_native_input_coverage(&native::product(), 20_000_000);
    root_input::assert_native_input_coverage(
        &native::finite_with(native::spec(8, 4, 256)),
        20_000_000,
    );
}
