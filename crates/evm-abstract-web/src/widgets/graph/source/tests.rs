use super::{ending, instruction, title};

#[test]
fn source_block_range_and_last_opcode_do_not_relabel_the_start_pc() {
    let push = evm_abstract_protocol::DisasmInstruction {
        pc: 0,
        opcode: 0x60,
        name: "PUSH1".into(),
        immediate: Some("0x40".into()),
        size: 2,
        valid: true,
        stack_inputs: 0,
        stack_outputs: 1,
    };
    let jump = evm_abstract_protocol::DisasmInstruction {
        pc: 12,
        opcode: 0x57,
        name: "JUMPI".into(),
        immediate: None,
        size: 1,
        valid: true,
        stack_inputs: 2,
        stack_outputs: 0,
    };
    let code = [push, jump];
    assert_eq!(title(0, &code), "Block 0x0000–0x000c");
    assert_eq!(ending(&code), "JUMPI @ 0x000c");
    assert_eq!(instruction(&code[0]), "0000  PUSH1 0x40");
    assert!(!ending(&code).ends_with("0x0000"));
}

#[test]
fn an_empty_source_range_has_no_fabricated_opcode() {
    assert_eq!(title(17, &[]), "Block 0x0011");
    assert_eq!(ending(&[]), "Decoded instructions unavailable");
}
