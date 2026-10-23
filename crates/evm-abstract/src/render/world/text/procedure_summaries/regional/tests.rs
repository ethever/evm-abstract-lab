use super::write;
use crate::analysis::{
    EvmRegionalCall, EvmRegionalCfg, EvmRegionalEdge, EvmRegionalNode, EvmRegionalReturn,
    EvmRegionalTransfer, FrameCode, FrameKey, MachineEdgeKind, OutcomeKind,
};
use alloy_primitives::{Address, keccak256};

#[test]
fn non_program_frame_identity_hashes_do_not_claim_captured_bytes_or_blocks() {
    let mut bytes = [0; 20];
    bytes[19] = 4;
    let address = Address::from(bytes);
    let mut indicator = vec![0xef, 0x01, 0x00];
    indicator.extend_from_slice(address.as_slice());
    for (mode, hash) in [
        (FrameCode::Precompile(address), keccak256([])),
        (FrameCode::Empty, keccak256([])),
        (FrameCode::InvalidDelegation, keccak256(indicator)),
    ] {
        let cfg = EvmRegionalCfg {
            nodes: vec![EvmRegionalNode {
                id: 0,
                procedure: 0,
                frame: FrameKey {
                    code_address: address,
                    code_hash: hash,
                    mode,
                    address,
                    address_value: address.into(),
                    caller: Address::repeat_byte(0x11).into(),
                    is_static: false,
                    basic_block_index: 0,
                    stack_height: 0,
                    jump_history: vec![],
                },
                input_revision: 1,
                excluded: false,
                successors: vec![],
                transfer: None,
            }],
            calls: vec![],
        };
        let mut output = String::new();
        write(&mut output, &cfg, true, false);
        assert!(
            output.contains(&format!("frame identity hash={hash}")),
            "{output}"
        );
        assert!(output.contains(&format!("{mode:?}")), "{output}");
        assert!(!output.contains("captured code hash=") && !output.contains("program block="));
    }
}

#[test]
fn source_current_gates_text_transfers_and_deliveries_without_erasing_local_evidence() {
    let address = Address::repeat_byte(0x11);
    let cfg = EvmRegionalCfg {
        nodes: vec![EvmRegionalNode {
            id: 0,
            procedure: 0,
            frame: FrameKey {
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
            id: 0,
            from: 0,
            callee: 0,
            entries: vec![0],
            input_revision: 1,
            deliveries: vec![EvmRegionalReturn {
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
