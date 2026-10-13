use super::*;
#[test]
fn exact_payload_and_large_copy_labels_stay_bounded() {
    let exact = api::Expression::BytesExact(format!("0x{}", "aa".repeat(24 * 1024)));
    let text = label(&exact);
    assert!(text.contains("24576 bytes"));
    assert!(text.len() < 200);
    let copy = api::Expression::Copy(api::ByteCopy {
        previous: api::ByteStateRef { node: 0 },
        destination: api::ExpressionRef { node: 1 },
        source: api::ByteViewRef { node: 3 },
        length: api::ExpressionRef { node: 2 },
    });
    assert!(label(&copy).contains("captured source"));
    assert_eq!(copy.references().len(), 4);
}
#[test]
fn shared_reachable_graph_is_iterative_and_unique() {
    let mut graph = crate::tests::symbolic_graph();
    let mut previous = api::ExpressionRef { node: 1 };
    for _ in 0..4096 {
        let node = graph.nodes.len();
        graph
            .nodes
            .push(api::Expression::Operation(api::ExpressionOperation {
                opcode: 0x01,
                arguments: vec![previous, previous],
            }));
        previous = api::ExpressionRef { node };
    }
    graph.validate().unwrap();
    let rows = reachable(&graph, previous.node);
    assert_eq!(rows.len(), 4097);
    assert!(rows.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn actual_symbolic_copy_and_storage_reports_survive_json_and_frontend_installation() {
    for code in ["366000803700", "5f3580600790555400"] {
        let request = api::AnalyzeRequest {
            input: api::AnalysisInput::Bytecode(api::BytecodeInput {
                bytecode: code.into(),
                address: Some(format!("0x{}", "11".repeat(20))),
            }),
            ..api::AnalyzeRequest::default()
        };
        let report = evm_abstract_server::analyze::analyze(request).unwrap();
        let wire = serde_json::to_vec(&api::AnalyzeReply { result: Ok(report) }).unwrap();
        let reply: api::AnalyzeReply = serde_json::from_slice(&wire).unwrap();
        let parsed = reply.result.as_ref().unwrap();
        parsed.validate().unwrap();
        let mut workspace = crate::Workspace::default();
        workspace.receive(Ok(reply));
        let installed = workspace
            .report
            .as_ref()
            .expect("validated report must enter frontend model");
        if code.starts_with("36") {
            let copy = installed
                .symbolic
                .nodes
                .iter()
                .find(|node| matches!(node, api::Expression::Copy(_)))
                .unwrap();
            assert!(label(copy).contains("captured source"));
            let range = installed
                .frontiers
                .iter()
                .find_map(|f| f.range.as_ref())
                .unwrap();
            assert!(installed.pending_machines.get(range.payload).is_some());
            assert!(
                installed
                    .ssa
                    .blocks
                    .iter()
                    .flat_map(|block| &block.instructions)
                    .any(|instruction| instruction.opcode == 0x37
                        && instruction.progress == api::InstructionProgress::Completed)
            );
        } else {
            assert!(
                installed
                    .states
                    .iter()
                    .filter_map(|s| s.exit.as_ref())
                    .any(|s| s.frames.last().is_some_and(|f| f
                        .stack
                        .last()
                        .is_some_and(|v| v.constants == Some(vec!["0x7".into()]))))
            );
            let store = installed
                .symbolic
                .nodes
                .iter()
                .find(|node| matches!(node, api::Expression::Store(_)))
                .unwrap();
            assert!(label(store).contains("ordered update"));
            assert!(installed.ssa.values.iter().any(|v| {
                matches!(&v.origin,api::SsaValueOrigin::Instruction(origin) if origin.pc==7)
                    && v.facts.iter().any(|fact| {
                        fact.status == api::SsaFactStatus::Current
                            && fact.constant.as_deref() == Some("0x7")
                    })
            }));
        }
    }
}
