use super::write;
use crate::analysis::{
    EvmRegionalCall, EvmRegionalCfg, EvmRegionalEdge, EvmRegionalNode, EvmRegionalReturn,
    EvmRegionalTransfer, FrameCode, FrameKey, MachineEdgeKind, OutcomeKind,
};
use alloy_primitives::{Address, keccak256};

#[test]
fn source_current_gates_text_transfers_and_deliveries_without_erasing_local_evidence() {
    let address = Address::repeat_byte(0x11);
    let cfg = EvmRegionalCfg {
        nodes: vec![EvmRegionalNode {
            id: 0,
            procedure: 0,
            frame: FrameKey {
                process: None,
                code_address: address,
                code_hash: keccak256([0]),
                mode: FrameCode::Runtime,
                address,
                address_value: address.into(),
                caller: address.into(),
                is_static: false,
                basic_block_index: 0,
                stack_height: 0,
                jump_history: vec![],
            },
            input_revision: 1,
            excluded: false,
            successors: vec![0],
            transfer: Some(EvmRegionalTransfer {
                semantics: crate::analysis::TransferSemantics::Native,
                process: None,
                input_revision: 1,
                current: true,
                complete: true,
                visited_block: true,
                successors: vec![EvmRegionalEdge {
                    to: 0,
                    kind: MachineEdgeKind::Call,
                }],
                calls: vec![0],
                outcomes: vec![],
            }),
        }],
        calls: vec![EvmRegionalCall {
            scope: None,
            action: None,
            target: None,
            id: 0,
            from: 0,
            callee: 0,
            entries: vec![0],
            input_revision: 1,
            deliveries: vec![EvmRegionalReturn {
                creation: Vec::new(),
                kind: OutcomeKind::Return,
                input_revision: 1,
                output_generation: 1,
                current: true,
                successors: vec![EvmRegionalEdge {
                    to: 0,
                    kind: MachineEdgeKind::Return,
                }],
            }],
        }],
    };
    for (source_current, superseded, expected) in [
        (false, false, "uncertified source"),
        (false, true, "historical observation"),
        (true, false, "current"),
    ] {
        let mut output = String::new();
        write(&mut output, &cfg, source_current, superseded);
        assert!(output.contains(&format!("{expected} transfer:")));
        assert!(output.contains(&format!("{expected} delivery:")));
        assert!(output.contains("record-local current=true"));
        if !source_current {
            assert!(!output.contains("current transfer:") && !output.contains("current delivery:"));
        }
    }
}
