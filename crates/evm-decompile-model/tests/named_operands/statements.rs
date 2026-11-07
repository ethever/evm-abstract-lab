use super::super::{SENTINELS, assert_statements, primitive};
use evm_decompile_model::EntityId;

#[test]
fn primitive_and_terminal_consumers_share_one_qualified_spelling() {
    use evm_decompile_model::ExitKind;
    for (opcode, name, arity, kind, expected) in [
        (0x00, "STOP", 0, ExitKind::Stop, "evm.stop();"),
        (
            0xf3,
            "RETURN",
            2,
            ExitKind::Return,
            "evm.return(offset = 0x11, size = 0x22);",
        ),
        (
            0xfd,
            "REVERT",
            2,
            ExitKind::Revert,
            "evm.revert(offset = 0x11, size = 0x22);",
        ),
        (
            0xff,
            "SELFDESTRUCT",
            1,
            ExitKind::SelfDestruct,
            "evm.selfdestruct(beneficiary = 0x11);",
        ),
    ] {
        let (program, ir, statement) = primitive(opcode, name, arity);
        let original = (program.clone(), ir.clone());
        let renderer = program.render_index(&ir);
        assert_eq!(
            renderer
                .render_exit(kind, &statement.inputs, |_| true)
                .unwrap(),
            expected
        );
        for state in [false, true] {
            assert_eq!(
                renderer
                    .render_statement_with_dependencies(&statement, state, |_| true)
                    .unwrap(),
                expected
            );
        }
        for spelling in [
            name.to_lowercase(),
            format!("evm.{}", name.to_lowercase()),
            format!("evm.evm.{}", name.to_lowercase()),
        ] {
            assert_eq!(
                renderer
                    .render_invocation(opcode, &spelling, &statement.inputs, |_| true)
                    .unwrap(),
                expected.strip_suffix(';').unwrap()
            );
        }
        assert_eq!((program, ir), original);
    }
    let (program, ir, _) = primitive(0x00, "STOP", 0);
    assert_eq!(
        program
            .render_index(&ir)
            .render_exit(ExitKind::EndOfCode, &[], |_| true)
            .unwrap(),
        "evm.stop(); // end of code"
    );
}

#[test]
fn external_calls_keep_seven_or_six_distinct_pop_order_operands() {
    assert_statements(&[
        (
            0xf1,
            "CALL",
            7,
            "evm.call(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            0xf2,
            "CALLCODE",
            7,
            "evm.callcode(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            0xf4,
            "DELEGATECALL",
            6,
            "evm.delegatecall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
        ),
        (
            0xfa,
            "STATICCALL",
            6,
            "evm.staticcall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
        ),
        (
            0xf0,
            "CREATE",
            3,
            "evm.create(value = 0x11, init_offset = 0x22, init_size = 0x33);",
        ),
        (
            0xf5,
            "CREATE2",
            4,
            "evm.create2(value = 0x11, init_offset = 0x22, init_size = 0x33, salt = 0x44);",
        ),
    ]);
}

#[test]
fn memory_storage_hash_and_copy_labels_identify_each_value() {
    assert_statements(&[
        (
            0x20,
            "KECCAK256",
            2,
            "evm.keccak256(offset = 0x11, size = 0x22);",
        ),
        (0x31, "BALANCE", 1, "evm.balance(address = 0x11);"),
        (0x35, "CALLDATALOAD", 1, "evm.calldataload(offset = 0x11);"),
        (
            0x37,
            "CALLDATACOPY",
            3,
            "evm.calldatacopy(memory_offset = 0x11, calldata_offset = 0x22, size = 0x33);",
        ),
        (
            0x39,
            "CODECOPY",
            3,
            "evm.codecopy(memory_offset = 0x11, code_offset = 0x22, size = 0x33);",
        ),
        (0x3b, "EXTCODESIZE", 1, "evm.extcodesize(address = 0x11);"),
        (
            0x3c,
            "EXTCODECOPY",
            4,
            "evm.extcodecopy(address = 0x11, memory_offset = 0x22, code_offset = 0x33, size = 0x44);",
        ),
        (
            0x3e,
            "RETURNDATACOPY",
            3,
            "evm.returndatacopy(memory_offset = 0x11, returndata_offset = 0x22, size = 0x33);",
        ),
        (0x3f, "EXTCODEHASH", 1, "evm.extcodehash(address = 0x11);"),
        (0x40, "BLOCKHASH", 1, "evm.blockhash(number = 0x11);"),
        (0x49, "BLOBHASH", 1, "evm.blobhash(index = 0x11);"),
        (0x51, "MLOAD", 1, "evm.mload(offset = 0x11);"),
        (
            0x52,
            "MSTORE",
            2,
            "evm.mstore(offset = 0x11, value = 0x22);",
        ),
        (
            0x53,
            "MSTORE8",
            2,
            "evm.mstore8(offset = 0x11, value = 0x22);",
        ),
        (0x54, "SLOAD", 1, "evm.sload(slot = 0x11);"),
        (0x55, "SSTORE", 2, "evm.sstore(slot = 0x11, value = 0x22);"),
        (0x5c, "TLOAD", 1, "evm.tload(slot = 0x11);"),
        (0x5d, "TSTORE", 2, "evm.tstore(slot = 0x11, value = 0x22);"),
        (
            0x5e,
            "MCOPY",
            3,
            "evm.mcopy(dest_offset = 0x11, src_offset = 0x22, size = 0x33);",
        ),
    ]);
}

#[test]
fn log_topics_follow_pop_order_without_collapsing_equal_looking_positions() {
    assert_statements(&[
        (0xa0, "LOG0", 2, "evm.log0(offset = 0x11, size = 0x22);"),
        (
            0xa1,
            "LOG1",
            3,
            "evm.log1(offset = 0x11, size = 0x22, topic0 = 0x33);",
        ),
        (
            0xa2,
            "LOG2",
            4,
            "evm.log2(offset = 0x11, size = 0x22, topic0 = 0x33, topic1 = 0x44);",
        ),
        (
            0xa3,
            "LOG3",
            5,
            "evm.log3(offset = 0x11, size = 0x22, topic0 = 0x33, topic1 = 0x44, topic2 = 0x55);",
        ),
        (
            0xa4,
            "LOG4",
            6,
            "evm.log4(offset = 0x11, size = 0x22, topic0 = 0x33, topic1 = 0x44, topic2 = 0x55, topic3 = 0x66);",
        ),
    ]);
}

#[test]
fn arithmetic_statement_forms_use_the_contract_even_when_normally_inlined() {
    assert_statements(&[
        (0x01, "ADD", 2, "evm.add(left = 0x11, right = 0x22);"),
        (0x02, "MUL", 2, "evm.mul(left = 0x11, right = 0x22);"),
        (0x03, "SUB", 2, "evm.sub(left = 0x11, right = 0x22);"),
        (0x04, "DIV", 2, "evm.div(dividend = 0x11, divisor = 0x22);"),
        (
            0x05,
            "SDIV",
            2,
            "evm.sdiv(dividend = 0x11, divisor = 0x22);",
        ),
        (0x06, "MOD", 2, "evm.mod(dividend = 0x11, divisor = 0x22);"),
        (
            0x07,
            "SMOD",
            2,
            "evm.smod(dividend = 0x11, divisor = 0x22);",
        ),
        (
            0x08,
            "ADDMOD",
            3,
            "evm.addmod(left = 0x11, right = 0x22, modulus = 0x33);",
        ),
        (
            0x09,
            "MULMOD",
            3,
            "evm.mulmod(left = 0x11, right = 0x22, modulus = 0x33);",
        ),
        (0x0a, "EXP", 2, "evm.exp(base = 0x11, exponent = 0x22);"),
        (
            0x0b,
            "SIGNEXTEND",
            2,
            "evm.signextend(byte_index = 0x11, value = 0x22);",
        ),
        (0x10, "LT", 2, "evm.lt(left = 0x11, right = 0x22);"),
        (0x11, "GT", 2, "evm.gt(left = 0x11, right = 0x22);"),
        (0x12, "SLT", 2, "evm.slt(left = 0x11, right = 0x22);"),
        (0x13, "SGT", 2, "evm.sgt(left = 0x11, right = 0x22);"),
        (0x14, "EQ", 2, "evm.eq(left = 0x11, right = 0x22);"),
        (0x15, "ISZERO", 1, "evm.iszero(value = 0x11);"),
        (0x16, "AND", 2, "evm.and(left = 0x11, right = 0x22);"),
        (0x17, "OR", 2, "evm.or(left = 0x11, right = 0x22);"),
        (0x18, "XOR", 2, "evm.xor(left = 0x11, right = 0x22);"),
        (0x19, "NOT", 1, "evm.not(value = 0x11);"),
        (0x1a, "BYTE", 2, "evm.byte(index = 0x11, value = 0x22);"),
        (0x1b, "SHL", 2, "evm.shl(shift = 0x11, value = 0x22);"),
        (0x1c, "SHR", 2, "evm.shr(shift = 0x11, value = 0x22);"),
        (0x1d, "SAR", 2, "evm.sar(shift = 0x11, value = 0x22);"),
        (0x1e, "CLZ", 1, "evm.clz(value = 0x11);"),
    ]);
}

#[test]
fn exits_and_retained_control_operands_have_meaningful_labels() {
    assert_statements(&[
        (0x50, "POP", 1, "evm.pop(value = 0x11);"),
        (0x56, "JUMP", 1, "evm.jump(target = 0x11);"),
        (
            0x57,
            "JUMPI",
            2,
            "evm.jumpi(target = 0x11, condition = 0x22);",
        ),
        (0xf3, "RETURN", 2, "evm.return(offset = 0x11, size = 0x22);"),
        (0xfd, "REVERT", 2, "evm.revert(offset = 0x11, size = 0x22);"),
        (
            0xff,
            "SELFDESTRUCT",
            1,
            "evm.selfdestruct(beneficiary = 0x11);",
        ),
        (0x00, "STOP", 0, "evm.stop();"),
        (0x30, "ADDRESS", 0, "evm.address();"),
        (0x36, "CALLDATASIZE", 0, "evm.calldatasize();"),
        (0x5f, "PUSH0", 0, "evm.push0();"),
        (0x60, "PUSH1", 0, "evm.push1();"),
        (0x80, "DUP1", 0, "evm.dup1();"),
        (0x90, "SWAP1", 0, "evm.swap1();"),
    ]);
}

#[test]
fn unknown_and_missing_or_excess_operands_remain_wholly_positional() {
    for (opcode, name, arity) in [
        (0x55, "SSTORE", 2),
        (0x3c, "EXTCODECOPY", 4),
        (0xa4, "LOG4", 6),
        (0xf1, "CALL", 7),
        (0xf2, "CALLCODE", 7),
        (0xf4, "DELEGATECALL", 6),
        (0xfa, "STATICCALL", 6),
        (0xf5, "CREATE2", 4),
        (0xfd, "REVERT", 2),
    ] {
        for count in [0, arity - 1, arity + 1] {
            let (program, ir, statement) = primitive(opcode, name, count);
            let operands = SENTINELS[..count]
                .iter()
                .map(|value| format!("0x{value:x}"))
                .collect::<Vec<_>>()
                .join(", ");
            assert_eq!(
                program.render_statement(&statement, &ir),
                format!("evm.{}({operands});", name.to_lowercase()),
                "{name} with {count} supplied operands"
            );
        }
    }
    assert_statements(&[
        // A recognized mnemonic must not select the signature of another opcode.
        (
            0x0c,
            "CALL",
            7,
            "evm.call(0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77);",
        ),
        (0x0c, "UNKNOWN", 0, "evm.unknown();"),
        (
            0x0c,
            "UNKNOWN",
            8,
            "evm.unknown(0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88);",
        ),
        (0x00, "STOP", 1, "evm.stop(0x11);"),
        (0x60, "PUSH1", 2, "evm.push1(0x11, 0x22);"),
        (0x80, "DUP1", 1, "evm.dup1(0x11);"),
        (0x90, "SWAP1", 2, "evm.swap1(0x11, 0x22);"),
        (
            0x55,
            "VENDOR_STORE",
            2,
            "evm.vendor_store(slot = 0x11, value = 0x22);",
        ),
    ]);
}

#[test]
fn output_bindings_and_dependency_metadata_do_not_change_named_values() {
    let (program, mut ir, mut statement) = primitive(0xf1, "CALL", 7);
    statement.outputs = vec![EntityId(911), EntityId(922)];
    let operation = &mut ir.blocks[0].operations[0];
    operation.memory_input = Some(EntityId(611));
    operation.memory_output = Some(EntityId(622));
    operation.effect_input = Some(EntityId(633));
    operation.effect_output = Some(EntityId(644));
    let original = (program.clone(), ir.clone(), statement.clone());
    let body = "v911, v922 = evm.call(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);";
    let full = format!("{body} // memory m611 → m622 // effect q633 → q644");
    let renderer = program.render_index(&ir);
    assert_eq!(
        renderer.render_statement(&statement, |_| true).unwrap(),
        full
    );
    assert_eq!(program.render_statement(&statement, &ir), full);
    assert_eq!(
        renderer
            .render_statement_with_dependencies(&statement, false, |_| true)
            .unwrap(),
        body
    );
    assert_eq!(
        renderer
            .render_statement_with_dependencies(&statement, true, |_| true)
            .unwrap(),
        full
    );
    assert_eq!((program, ir, statement), original);
}
