//! Epoch validity must not depend on whether an earlier opaque query warmed a
//! memo. The three control engines still execute every source observation.
use evm_decompile::{
    Completion, Config, ExecutionLimits, ExitKind, ExternalResponse, Fork, Input,
    LiftedByteSegment, LiftedByteSource, LiftedDataWord, LiftedRevertDataKind, MemoryHost, U256,
    ValidationStatus, decompile, execute_bytecode, execute_ir, execute_structured,
};

fn call(code: &mut Vec<u8>) {
    code.extend([0x5f; 5]);
    code.extend([0x61, 0x12, 0x34, 0x62, 1, 0, 0, 0xf1, 0x50]);
}
fn reads(code: &mut Vec<u8>, count: usize) {
    for _ in 0..count {
        code.extend([0x3d, 0x50]);
    }
}
fn copy(code: &mut Vec<u8>) {
    code.extend([0x60, 1, 0x5f, 0x5f, 0x3e]);
}
fn raw_exit(code: &mut Vec<u8>) {
    copy(code);
    code.extend([0x3d, 0x5f, 0xfd]);
}
fn exact_exit(code: &mut Vec<u8>) {
    copy(code);
    code.extend([0x60, 1, 0x5f, 0xfd]);
}

fn fixture(raw_first: Option<bool>) -> Input {
    let mut code = Vec::new();
    call(&mut code);
    if let Some(raw_first) = raw_first {
        reads(&mut code, 100);
        code.extend([0x5f, 0x35, 0x61, 0, 0, 0x57]);
        let fix = code.len() - 3;
        if raw_first {
            raw_exit(&mut code);
        } else {
            reads(&mut code, 200);
            exact_exit(&mut code);
        }
        let destination = u16::try_from(code.len()).unwrap();
        code.push(0x5b);
        if raw_first {
            reads(&mut code, 200);
            exact_exit(&mut code);
        } else {
            raw_exit(&mut code);
        }
        code[fix..fix + 2].copy_from_slice(&destination.to_be_bytes());
    } else {
        reads(&mut code, 300);
        exact_exit(&mut code);
    }
    Input::new(code, Fork::Osaka)
}

#[test]
fn deep_returndata_epochs_replay_with_cold_warm_and_reordered_raw_queries() {
    for order in [Some(true), Some(false), None] {
        let input = fixture(order);
        let artifact = decompile(&input, &Config::default());
        assert_eq!(
            artifact.completion,
            Completion::Complete,
            "order={order:?}: {:?}",
            artifact.diagnostics
        );
        assert!(
            artifact
                .stages
                .iter()
                .all(|stage| stage.validation.status == ValidationStatus::Passed)
        );
        let stage = artifact.current().unwrap();
        let high = stage.ir.lifted.as_ref().unwrap();
        let slices: Vec<_> = high
            .revert_data
            .iter()
            .filter_map(|data| {
                if let LiftedRevertDataKind::Segments(parts) = &data.content
                    && let [
                        LiftedByteSegment::Slice {
                            source: LiftedByteSource::ReturnData(call),
                            offset: LiftedDataWord::Constant(offset),
                            size: LiftedDataWord::Constant(size),
                        },
                    ] = parts.as_slice()
                {
                    assert_eq!((*offset, *size), (U256::ZERO, U256::from(1)));
                    assert_eq!(data.writes.len(), 1);
                    return Some(*call);
                }
                None
            })
            .collect();
        assert_eq!(slices.len(), 1, "order={order:?}");
        assert_eq!(
            high.revert_data
                .iter()
                .filter(|data| matches!(data.content, LiftedRevertDataKind::Raw))
                .count(),
            usize::from(order.is_some())
        );
        for word in [U256::ZERO, U256::from(1)] {
            let mut initial = MemoryHost::new(&input, word.to_be_bytes::<32>().to_vec());
            initial.responses.push_back(ExternalResponse {
                result: U256::from(1),
                return_data: vec![0x42, 0x43, 0x44],
                ..ExternalResponse::default()
            });
            let limits = ExecutionLimits::default();
            let mut raw_host = initial.clone();
            let raw = execute_bytecode(&input, &mut raw_host, &limits).unwrap();
            assert!(matches!(
                raw.outcome,
                evm_decompile::ExecutionOutcome::Exit {
                    kind: ExitKind::Revert,
                    ..
                }
            ));
            let mut graph_host = initial.clone();
            let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
            let mut tree_host = initial;
            let tree = execute_structured(stage, &mut tree_host, &limits).unwrap();
            assert_eq!(graph.outcome, raw.outcome);
            assert_eq!(tree.outcome, raw.outcome);
            assert_eq!(graph.observations(), raw.observations());
            assert_eq!(tree.observations(), raw.observations());
            assert_eq!(graph_host, raw_host);
            assert_eq!(tree_host, raw_host);
        }
    }
}
