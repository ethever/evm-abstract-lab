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
    for (code, max_nodes) in [
        ("366000803700", None),
        ("366000803700", Some(3)),
        ("5f3580600790555400", None),
    ] {
        let mut request = api::AnalyzeRequest {
            input: api::AnalysisInput::Bytecode(api::BytecodeInput {
                bytecode: code.into(),
                address: Some(format!("0x{}", "11".repeat(20))),
            }),
            ..api::AnalyzeRequest::default()
        };
        if let Some(max_nodes) = max_nodes {
            // Keep the strict three-node range-refusal fixture independent of
            // optional terminal preparation and its prepaid expression work.
            request.limits.root_terminal_coverage = false;
            request.limits.max_expression_nodes = max_nodes;
        }
        let report = crate::tests::native_component(
            (evm_abstract_server::analyze::analyze(request).unwrap()).into(),
        );
        let wire = serde_json::to_vec(&api::AnalyzeReply {
            result: Ok((report).into()),
        })
        .unwrap();
        let reply: api::AnalyzeReply = serde_json::from_slice(&wire).unwrap();
        let parsed = reply.result.as_ref().unwrap();
        parsed.validate().unwrap();
        let expected = parsed.clone();
        let mut workspace = crate::Workspace::default();
        workspace.receive(Ok(reply));
        let installed = workspace
            .report
            .as_ref()
            .expect("validated report must enter frontend model");
        assert_eq!(installed, &expected);
        if code.starts_with("36") {
            let state = &installed.selected_graph().unwrap().states[0];
            let exit = state.exit.as_ref().unwrap();
            let frame = exit.frames.last().unwrap();
            let input = &installed.selected_graph().unwrap().byte_arrays[frame.calldata.unwrap()];
            let length = input.length.expression.unwrap();
            assert!(
                matches!(&installed.selected_graph().unwrap().symbolic.nodes[length.node], api::Expression::Input(identity)
                if identity.symbol.kind == api::InputSymbolKind::CalldataLength)
            );
            let instruction = installed.selected_graph().unwrap().ssa.blocks[0]
                .instructions
                .iter()
                .find(|instruction| instruction.pc == 4)
                .unwrap();
            assert_eq!(instruction.opcode, 0x37);
            assert_eq!(instruction.operands.len(), 3);
            assert!(instruction.results.is_empty());
            assert!(instruction.effect_result.is_some());
            if max_nodes.is_none() {
                assert_eq!(installed.status, api::AnalysisStatus::Converged);
                assert!(installed.selected_graph().unwrap().frontiers.is_empty());
                assert!(installed.selected_graph().unwrap().ssa.complete);
                let api::Expression::View(memory) = &installed
                    .selected_graph()
                    .unwrap()
                    .symbolic
                    .nodes[installed.selected_graph().unwrap().byte_arrays[frame.memory.unwrap()]
                    .symbolic
                    .node]
                else {
                    panic!("installed memory retains its view")
                };
                let copy = &installed.selected_graph().unwrap().symbolic.nodes[memory.state.node];
                assert!(label(copy).contains("captured source"));
                let api::Expression::Copy(copy) = copy else {
                    panic!("installed success commits the lazy copy")
                };
                assert_eq!(copy.length, length);
                assert!(matches!(
                    installed.selected_graph().unwrap().symbolic.nodes[copy.previous.node],
                    api::Expression::BytesZero
                ));
                assert_eq!(
                    installed.selected_graph().unwrap().symbolic.nodes[copy.destination.node],
                    api::Expression::Constant("0x0".into())
                );
                let api::Expression::View(source) =
                    &installed.selected_graph().unwrap().symbolic.nodes[copy.source.node]
                else {
                    panic!("installed copy retains its captured source")
                };
                let api::Expression::View(input) =
                    &installed.selected_graph().unwrap().symbolic.nodes[input.symbolic.node]
                else {
                    panic!("installed calldata retains its input view")
                };
                assert_eq!(source.state, input.state);
                assert_eq!(source.offset, input.offset);
                assert_eq!(source.length, length);
                assert!(matches!(
                    installed.selected_graph().unwrap().symbolic.nodes[exit.path.node],
                    api::Expression::PathAnd(_)
                ));
                assert_eq!(instruction.progress, api::InstructionProgress::Completed);
            } else {
                // Three nodes admit calldata but cannot retain the complete
                // logical-memory guard. Its mandatory partition stays pending.
                assert_eq!(installed.status, api::AnalysisStatus::Incomplete);
                assert!(!installed.selected_graph().unwrap().ssa.complete);
                let [frontier] = installed.selected_graph().unwrap().frontiers.as_slice() else {
                    panic!("expected exactly the installed logical range refusal")
                };
                assert_eq!(frontier.kind, api::FrontierKind::Relations);
                assert_eq!(
                    frontier.reason,
                    api::FrontierDetails::Relations(api::QueryBoundary::ExpressionLimit)
                );
                assert_eq!((frontier.from, frontier.pc), (Some(0), Some(4)));
                let range = frontier.range.as_ref().unwrap();
                assert_eq!(range.request.kind, api::RangeKind::LogicalMemory);
                assert_eq!(range.request.offset.constants, Some(vec!["0x0".into()]));
                assert_eq!(range.request.size.expression, Some(length));
                assert!(range.request.size.constants.is_none());
                assert_eq!(
                    range.request.limit.constants,
                    Some(vec![format!("0x{}e0", "ff".repeat(31))])
                );
                assert_eq!(range.nonzero, None);
                assert!(range.condition.expression.is_none());
                let pending = &installed.selected_graph().unwrap().pending_machines[range.payload];
                assert_eq!(pending.frames, exit.frames);
                assert_eq!(pending.store, exit.store);
                assert_eq!(exit.store, state.entry.store);
                assert!(frame.stack.is_empty());
                let memory =
                    &installed.selected_graph().unwrap().byte_arrays[frame.memory.unwrap()];
                assert_eq!(memory.length.constants, Some(vec!["0x0".into()]));
                let api::Expression::View(view) =
                    &installed.selected_graph().unwrap().symbolic.nodes[memory.symbolic.node]
                else {
                    panic!("installed refusal retains the pre-effect memory view")
                };
                assert!(matches!(
                    installed.selected_graph().unwrap().symbolic.nodes[view.state.node],
                    api::Expression::BytesZero
                ));
                assert_eq!(
                    instruction.progress,
                    api::InstructionProgress::OperandsConsumed
                );
                assert!(matches!(
                    instruction.operation,
                    api::SsaOperation::Opcode(_)
                ));
                assert!(!installed.selected_graph().unwrap().ssa.values.iter().any(|value| {
                    matches!(&value.origin, api::SsaValueOrigin::Instruction(origin) if origin.pc == 4)
                }));
                assert!(
                    !installed.selected_graph().unwrap().ssa.blocks[0]
                        .instructions
                        .iter()
                        .any(|instruction| instruction.pc > 4)
                );
                assert!(
                    installed
                        .selected_graph()
                        .unwrap()
                        .symbolic
                        .nodes
                        .iter()
                        .all(|node| !matches!(node, api::Expression::Copy(_)))
                );
                assert!(
                    installed
                        .selected_graph()
                        .unwrap()
                        .outcomes
                        .iter()
                        .all(|outcome| outcome.kind == api::OutcomeKind::Failure)
                );
            }
        } else {
            assert_eq!(installed.status, api::AnalysisStatus::Converged);
            assert!(installed.selected_graph().unwrap().frontiers.is_empty());
            assert!(
                installed
                    .selected_graph()
                    .unwrap()
                    .states
                    .iter()
                    .filter_map(|s| s.exit.as_ref())
                    .any(|s| s.frames.last().is_some_and(|f| f
                        .stack
                        .last()
                        .is_some_and(|v| v.constants == Some(vec!["0x7".into()]))))
            );
            let store = installed
                .selected_graph()
                .unwrap()
                .symbolic
                .nodes
                .iter()
                .find(|node| matches!(node, api::Expression::Store(_)))
                .unwrap();
            assert!(label(store).contains("ordered update"));
            assert!(
                installed
                    .selected_graph()
                    .unwrap()
                    .ssa
                    .values
                    .iter()
                    .any(|v| {
                        matches!(&v.origin,api::SsaValueOrigin::Instruction(origin) if origin.pc==7)
                            && v.facts.iter().any(|fact| {
                                fact.status == api::SsaFactStatus::Current
                                    && fact.constant.as_deref() == Some("0x7")
                                    && fact.value.is_some()
                            })
                    })
            );
            assert!(
                installed
                    .selected_graph()
                    .unwrap()
                    .ssa
                    .values
                    .iter()
                    .any(|value| {
                        value.facts.iter().any(|fact| {
                            fact.status == api::SsaFactStatus::Current
                                && fact.scope == api::SsaFactScope::Definition
                                && fact.source == api::SsaFactSource::Literal
                                && fact.constant.as_deref() == Some("0x7")
                                && fact.value.is_none()
                        })
                    })
            );
        }
    }
}
