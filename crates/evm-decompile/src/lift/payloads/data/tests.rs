//! Producer boundaries independent of display format and certificate replay.
use super::{Memory, query};
use crate::{Config, Fork, Input, U256, decompile};
use std::cell::Cell;

#[test]
fn refused_query_returns_no_recovered_or_raw_completion() {
    let input = Input::new(
        evm_bytecode::parse_hex(include_str!(
            "../../../../tests/fixtures/revert-data/panic-41-guard.hex"
        ))
        .unwrap(),
        Fork::Osaka,
    );
    let artifact = decompile(&input, &Config::default());
    let ir = &artifact.current().unwrap().ir;
    let terminal = ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|op| op.opcode == 0xfd)
        .unwrap();
    let allow = Cell::new(true);
    let mut memory = Memory::new(ir, |_| allow.get()).unwrap();
    allow.set(false);
    assert!(
        query(
            &mut memory,
            terminal.memory_input.unwrap(),
            terminal.inputs[0],
            terminal.inputs[1]
        )
        .is_none()
    );
    assert!(memory.refused);
}

#[test]
fn unknown_loaded_content_keeps_identity_without_an_allocation_or_nonwrap_bound() {
    // Calldata overwrites the slot; its unknown bytes do not become a compiler
    // free-memory-pointer convention. The actual MLOAD occurrence remains P.
    let input = Input::new(
        vec![0x60, 128, 0x5f, 0x5f, 0x37, 0x60, 64, 0x51, 0x50, 0x00],
        Fork::Osaka,
    );
    let artifact = decompile(&input, &Config::default());
    let ir = &artifact.current().unwrap().ir;
    let load = ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|op| op.opcode == 0x51)
        .unwrap();
    let mut memory = Memory::new(ir, |_| true).unwrap();
    let word = memory
        .word(load.outputs[0])
        .expect("unknown content still has source identity");
    assert_eq!(
        word.base,
        Some(crate::lift::payloads::memory::Identity::Value(
            load.outputs[0]
        ))
    );
    assert_eq!((word.low, word.high), (U256::ZERO, U256::MAX));
    assert!(
        word.end(68).is_none(),
        "identity must not imply a nonwrapping allocation"
    );
}

#[test]
fn same_symbolic_word_xor_proves_zero_without_a_raw_constant_summary() {
    let input = Input::new(vec![0x5f, 0x35, 0x80, 0x18, 0x5f, 0xfd], Fork::Osaka);
    let artifact = decompile(&input, &Config::default());
    let mut ir = artifact.current().unwrap().ir.clone();
    let xor = ir
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.operations)
        .find(|op| op.opcode == 0x18)
        .unwrap();
    xor.replacement = None;
    let (operation, value) = (xor.id, xor.outputs[0]);
    let source = ir
        .values
        .iter_mut()
        .find(|source| source.id == value)
        .unwrap();
    source.kind = crate::ValueKind::Operation {
        operation,
        index: 0,
    };
    source.fact = crate::ValueFact::Unknown;
    let mut memory = Memory::new(&ir, |_| true).unwrap();
    assert_eq!(
        memory.word(value).and_then(super::Word::exact),
        Some(U256::ZERO)
    );
}

#[test]
fn returndata_epoch_uses_call_semantics_and_refuses_other_declared_writers() {
    let mut code = Vec::new();
    for _ in 0..2 {
        code.extend([0x5f; 7]);
        code.extend([0xf1, 0x50]);
    }
    code.extend([0x3d, 0x50, 0x00]);
    let artifact = decompile(&Input::new(code, Fork::Osaka), &Config::default());
    let mut ir = artifact.current().unwrap().ir.clone();
    let calls: Vec<_> = ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .filter(|op| op.opcode == 0xf1)
        .map(|op| (op.id, op.effect_output.unwrap()))
        .collect();
    assert_eq!(calls.len(), 2);
    let mut size_effect = None;
    for op in ir.blocks.iter_mut().flat_map(|block| &mut block.operations) {
        if op.id == calls[1].0 {
            op.effects.return_data_write = false;
        }
        if op.opcode == 0x3d {
            op.effects.return_data_write = true;
            size_effect = op.effect_output;
        }
    }
    let mut memory = Memory::new(&ir, |_| true).unwrap();
    assert_eq!(
        memory.return_data_call(calls[1].1),
        Some(calls[1].0),
        "a false flag cannot expose the older call's buffer"
    );
    assert_eq!(
        memory.return_data_call(size_effect.unwrap()),
        None,
        "an unsupported declared writer has no proved unchanged epoch"
    );
}
