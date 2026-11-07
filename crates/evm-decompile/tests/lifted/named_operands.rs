//! Real bytecode fixes operand order before the presentation layer is involved.
use super::oracle::certify;
use super::support::{Asm, checked};
use super::{assert_unchanged_witness, lifted, structured};
use evm_decompile::{
    Cancellation, ExternalResponse, MemoryHost, TextExportStatus, U256, export_text,
    export_text_with_observer, render_text,
};

#[test]
fn real_runtime_calls_export_names_without_changing_witness_or_semantics() {
    for (opcode, operands, expected) in [
        (
            0xf1,
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77][..],
            "evm.call(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            0xf2,
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77][..],
            "evm.callcode(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            0xf4,
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66][..],
            "evm.delegatecall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
        ),
        (
            0xfa,
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66][..],
            "evm.staticcall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
        ),
    ] {
        let mut asm = Asm::default();
        for &value in operands.iter().rev() {
            asm.push(value);
        }
        asm.op(opcode).op(0x50).op(0x00);
        let input = asm.finish();
        let artifact = checked(&input);
        let original = artifact.clone();
        let encoded = serde_json::to_vec(&artifact).unwrap();
        let (stage, program) = lifted(&artifact);
        let operation = stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .find(|operation| operation.opcode == opcode)
            .unwrap();
        let expected_values: Vec<_> = operands.iter().copied().map(U256::from).collect();
        let witnessed: Vec<_> = operation
            .inputs
            .iter()
            .map(|id| stage.ir.value(*id).unwrap().fact.singleton().unwrap())
            .collect();
        assert_eq!(witnessed, expected_values);
        let statement = program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find(|statement| statement.operation == operation.id)
            .unwrap();
        assert_eq!(statement.inputs.len(), expected_values.len());
        assert!(
            program
                .render_statement(statement, &stage.ir)
                .starts_with(expected)
        );
        let export = export_text(&artifact, &Cancellation::new());
        assert_eq!(export.status, TextExportStatus::Complete);
        assert!(export.text.contains(expected), "{}", export.text);

        let mut raw_artifact = artifact.clone();
        raw_artifact.current_stage = Some(structured(&artifact).id);
        let raw = render_text(&raw_artifact);
        let raw_operands = operands
            .iter()
            .map(|value| format!("0x{value:x}"))
            .collect::<Vec<_>>()
            .join(", ");
        let name = operation.name.to_lowercase();
        assert!(raw.contains(&format!("{name}({raw_operands});")), "{raw}");
        assert!(!raw.contains("gas = "), "{raw}");

        let mut host = MemoryHost::new(&input, Vec::new());
        host.responses.push_back(ExternalResponse::default());
        let (execution, observed) = certify(&input, &artifact, host);
        let calls: Vec<_> = execution
            .trace
            .iter()
            .filter(|step| step.opcode == opcode)
            .collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].inputs, expected_values);
        assert!(observed.responses.is_empty());
        assert_unchanged_witness(&artifact);
        assert_eq!(artifact, original);
        assert_eq!(serde_json::to_vec(&artifact).unwrap(), encoded);
    }
}

#[test]
fn named_invocation_export_refusal_and_cancellation_are_atomic() {
    let mut asm = Asm::default();
    asm.push(0x22).push(0x11).op(0x55).op(0x00);
    let artifact = checked(&asm.finish());
    let original = artifact.clone();
    let mut checkpoints = Vec::new();
    let complete = export_text_with_observer(&artifact, &Cancellation::new(), u64::MAX, |work| {
        checkpoints.push(work);
    });
    assert_eq!(complete.status, TextExportStatus::Complete);
    assert!(
        complete
            .text
            .contains("evm.sstore(slot = 0x11, value = 0x22);")
    );
    assert!(checkpoints.windows(2).all(|pair| pair[0] <= pair[1]));
    let mut interrupted_after_work = false;
    for allowance in checkpoints {
        let limited = export_text_with_observer(&artifact, &Cancellation::new(), allowance, |_| {});
        if limited.status == TextExportStatus::Complete {
            continue;
        }
        assert_eq!(limited.status, TextExportStatus::Limited);
        assert!(limited.work <= allowance);
        if !limited.text.contains("evm.sstore(") {
            interrupted_after_work |= limited.work > 0;
        } else {
            assert!(
                limited
                    .text
                    .contains("evm.sstore(slot = 0x11, value = 0x22);"),
                "{}",
                limited.text
            );
        }
        let cancel = Cancellation::new();
        let cancelled = export_text_with_observer(&artifact, &cancel, u64::MAX, |work| {
            if work >= allowance {
                cancel.cancel();
            }
        });
        assert!(cancel.is_cancelled());
        assert_eq!(cancelled.status, TextExportStatus::Cancelled);
        if cancelled.text.contains("evm.sstore(") {
            assert!(
                cancelled
                    .text
                    .contains("evm.sstore(slot = 0x11, value = 0x22);"),
                "{}",
                cancelled.text
            );
        }
    }
    assert!(
        interrupted_after_work,
        "exercise refusal after export has started"
    );
    assert_eq!(artifact, original);
}
