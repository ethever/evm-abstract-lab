#[test]
fn invalid_and_deferred_instructions_do_not_claim_observed_results() {
    let mut instruction =
        crate::tests::report().model.programs[0].blocks[0].instructions[0].clone();
    instruction.opcode = 0xfe;
    instruction.valid = true;
    let invalid = super::instruction(&instruction, false);
    assert!(invalid.contains("exceptional halt"));
    assert!(!invalid.contains("Stack signature"));
    instruction.opcode = 0x5f;
    instruction.valid = false;
    assert!(super::instruction(&instruction, false).contains("exceptional halt"));
    instruction.valid = true;
    for opcode in [0xf0, 0xf1, 0xf2, 0xf4, 0xf5, 0xfa] {
        instruction.opcode = opcode;
        let hover = super::instruction(&instruction, false);
        assert!(hover.contains("if the caller resumes"));
        assert!(hover.contains("incomplete analysis may not have this result yet"));
        assert!(!hover.contains("Observed in current execution evidence"));
    }
}
