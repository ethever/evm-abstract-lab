//! Byte-for-byte controls for the shared mechanical fixture builder.
#[path = "support/assembly.rs"]
mod assembly;

use alloy_primitives::U256;
use assembly::Assembly;
use revm_bytecode::opcode;

#[test]
fn forward_backward_and_data_labels_keep_the_original_push2_layout() {
    let mut code = Assembly::default();
    code.label("entry")
        .push(42)
        .jump("exit", true)
        .jump("entry", false)
        .data("raw", &[0xaa, 0xbb])
        .label("exit")
        .push_word(U256::MAX)
        .op(opcode::STOP);
    let result = code.finish();
    let mut expected = vec![
        0x5b, 0x61, 0, 42, 0x61, 0, 14, 0x57, 0x61, 0, 0, 0x56, 0xaa, 0xbb, 0x5b, 0x7f,
    ];
    expected.extend([0xff; 32]);
    expected.push(0);
    assert_eq!(result.bytes, expected);
    assert_eq!(
        result.labels.into_iter().collect::<Vec<_>>(),
        [("entry", 0), ("exit", 14), ("raw", 12)]
    );
}
