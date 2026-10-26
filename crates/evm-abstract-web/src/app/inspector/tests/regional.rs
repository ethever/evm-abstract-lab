//! Rendering consumes actual regional receipts, with no extra instruction/SSA data.
use super::*;

const RECURSIVE: &str = "5f5f5f5f5f305af150602a5f5260205ff3";

fn observation_report(bytecode: &str) -> crate::tests::AnalysisReport {
    let mut report = crate::tests::procedure_report(bytecode);
    // Rendering fixtures control the certificate. Native certification and the
    // exact exported flag are independently tested at the engine/server seam.
    report.abstract_summaries.truncate(1);
    report.abstract_summaries[0].source_current = true;
    report
}

#[test]
fn known_call_terminal_boundary_explains_region_evidence_without_asserting_recursion() {
    let mut report = crate::tests::report();
    report.frontiers = vec![api::Frontier {
        from: Some(0),
        pc: None,
        kind: api::FrontierKind::Model,
        reason: api::FrontierDetails::RecursiveNoTerminal,
        detail: "Known bytecode region has no covered terminal behavior".into(),
        range: None,
        payload: None,
    }];
    let context = context();
    let mut time = 0.0;
    let mut selection = Selection::default();
    let visible = rendered_at(
        &context,
        &mut time,
        vec![],
        egui::vec2(1400.0, 1200.0),
        |ui| {
            evidence::diagnostics(ui, &(report).view(), &mut selection);
        },
    );
    let text = visible
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Known bytecode region lacks terminal evidence"),
        "{text}"
    );
    assert!(!text.contains("Recursive execution"), "{text}");
    assert_eq!(
        serde_json::to_value(&report.frontiers[0].reason).unwrap(),
        "RecursiveNoTerminal"
    );
}

fn draw(
    context: &egui::Context,
    time: &mut f64,
    report: &crate::tests::AnalysisReport,
    events: Vec<egui::Event>,
) -> Vec<(String, egui::Rect)> {
    let source = report.abstract_summaries[0].source_state;
    rendered_at(context, time, events, egui::vec2(1800.0, 16000.0), |ui| {
        summaries::show(ui, &(report).view(), Some(source));
    })
}

fn click(
    context: &egui::Context,
    time: &mut f64,
    report: &crate::tests::AnalysisReport,
    label: &str,
) {
    let before = draw(context, time, report, vec![]);
    let position = before
        .iter()
        .find(|(text, _)| text == label)
        .unwrap_or_else(|| panic!("missing {label}: {before:?}"))
        .1
        .center();
    for pressed in [true, false] {
        draw(
            context,
            time,
            report,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

fn context() -> egui::Context {
    let context = egui::Context::default();
    crate::notation::initialize_fonts(&context);
    context.global_style_mut(|style| style.animation_time = 0.0);
    context
}

#[test]
fn conservative_process_shows_possible_static_control_exits_and_residual_obligations() {
    let mut report = observation_report(RECURSIVE);
    let top = crate::tests::empty_report()
        .metadata
        .environment
        .call_value
        .clone();
    // Reuse regional identities to exercise presentation. Native action and
    // target provenance is checked by the server's process integration test.
    let summary = &mut report.abstract_summaries[0];
    summary.kind = api::ProcedureSummaryKind::KnownProcess;
    summary.closed = false;
    for procedure in &mut summary.procedures {
        if procedure.kind == api::CodeKind::Runtime {
            let source = api::ProcessSource::Runtime(api::ProcessCapturedSource {
                source: procedure.code_address.take().unwrap(),
                hash: serde_json::from_value(serde_json::json!(
                    procedure.code_hash.take().unwrap()
                ))
                .unwrap(),
                fork: api::Fork::Osaka,
            });
            procedure.process = Some(api::ProcessFrameKey {
                epoch: api::ProcessDigest([0xa1; 32]),
                owner: api::ProcessOwner::Residual,
                source,
                is_static: procedure.is_static,
                location: api::ProcessLocation::Block(0),
            });
            procedure.process_input = Some(api::ProcessOperands {
                address: top.clone(),
                caller: top.clone(),
                value: top.clone(),
            });
            procedure.storage_address = None;
            procedure.semantics = api::TransferSemantics::KnownProcess;
            procedure.covered_blocks.clear();
        }
    }
    for node in &mut summary.regional_cfg.nodes {
        if summary.procedures[node.procedure].semantics != api::TransferSemantics::KnownProcess {
            continue;
        }
        node.frame.basic_block = None;
        node.frame.process = summary.procedures[node.procedure].process.clone();
        node.frame.code_address = None;
        node.frame.code_hash = None;
        node.frame.storage_address = None;
        node.frame.address_value = None;
        node.frame.caller = None;
        node.frame.stack_height = None;
        node.frame.context.clear();
        if let Some(transfer) = &mut node.transfer {
            transfer.semantics = api::TransferSemantics::KnownProcess;
            transfer.visited_block = false;
            transfer.complete = false;
            transfer.process = Some(api::ProcessTransfer {
                control: None,
                creations: vec![],
                repeat: true,
                actions: vec![api::ProcessAction::StaticCall],
                unresolved: vec![api::ProcessBoundary::ResidualOwner(
                    api::ProcessAction::StaticCall,
                )],
            });
        }
    }
    let node = summary
        .regional_cfg
        .nodes
        .iter()
        .find(|node| {
            node.transfer.as_ref().is_some_and(|transfer| {
                transfer.current && transfer.input_revision == node.input_revision && !node.excluded
            })
        })
        .unwrap();
    let heading = format!(
        "r{} · procedure {} · Current partial conservative process",
        node.id, node.procedure
    );
    let identity_hash = match &node.frame.process.as_ref().unwrap().source {
        api::ProcessSource::Runtime(source) => source.hash.to_string(),
        _ => unreachable!(),
    };
    for call in &mut summary.regional_cfg.calls {
        if summary.regional_cfg.nodes[call.from]
            .frame
            .process
            .is_none()
        {
            continue;
        }
        call.scope = Some(api::ProcessControlScope::Primitive);
        call.action = Some(api::ProcessAction::StaticCall);
        call.target = Some(api::ProcessOwner::Residual);
    }
    report.validate().unwrap();
    let context = context();
    let mut time = 0.0;
    click(&context, &mut time, &report, "Regional control flow");
    click(&context, &mut time, &report, &heading);
    click(&context, &mut time, &report, "Calls and returns");
    let text = draw(&context, &mut time, &report, vec![])
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Conservative runtime process",
        "Conservative process · possible control",
        "Represented actions: STATICCALL",
        "Possible action: STATICCALL",
        "Action target: Residual owner class · unresolved aliases",
        "Unexpanded: STATICCALL: remaining owners and aliases are unexpanded",
        "captured block visited: false",
        "recorded exits:",
        &format!("Captured code hash: {identity_hash}"),
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(!text.contains("Program block:"));
}

#[test]
fn source_current_withholds_final_labels_for_a_closed_current_local_observation() {
    let mut report = observation_report(RECURSIVE);
    let summary = &mut report.abstract_summaries[0];
    summary.closed = true;
    summary.source_current = false;
    let source = summary.source_state;
    let node_id = summary
        .regional_cfg
        .nodes
        .iter()
        .find(|node| {
            node.transfer
                .as_ref()
                .is_some_and(|transfer| transfer.current && transfer.complete)
        })
        .unwrap()
        .id;
    assert!(
        summary
            .regional_cfg
            .calls
            .iter()
            .flat_map(|call| &call.deliveries)
            .any(|delivery| delivery.current)
    );
    report.validate().unwrap();
    let selection = Selection {
        state: Some(source),
        ..Default::default()
    };
    let accessible = Inspector::default().accessible_summary(&(report).view(), selection);
    assert!(accessible.contains("closed for observed input · final source uncertified"));
    assert!(!accessible.contains("closed for final source") && !accessible.contains("historical"));
    let context = context();
    let mut time = 0.0;
    click(&context, &mut time, &report, "Regional control flow");
    let node = &report.abstract_summaries[0].regional_cfg.nodes[node_id];
    click(
        &context,
        &mut time,
        &report,
        &format!(
            "r{} · procedure {} · Final source uncertified",
            node.id, node.procedure
        ),
    );
    click(&context, &mut time, &report, "Calls and returns");
    let visible = draw(&context, &mut time, &report, vec![]);
    let text = visible
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Record-local transfer current: true",
        "Record-local delivery current: true",
        "Uncertified source transfer",
        "Uncertified source delivery",
        "final source uncertified",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    for forbidden in [
        "Current transfer:",
        "Current delivery:",
        "current for final source",
        "Historical observation",
    ] {
        assert!(!text.contains(forbidden), "{forbidden}: {text}");
    }
    for call in &report.abstract_summaries[0].regional_cfg.calls {
        assert!(text.contains(&format!("Call c{} · final source uncertified:", call.id)));
    }
}

#[test]
fn source_current_distinguishes_superseded_from_uncertified_latest_and_certified_latest() {
    let mut report = observation_report(RECURSIVE);
    let mut latest = report.abstract_summaries[0].clone();
    report.abstract_summaries[0].source_current = false;
    report.abstract_summaries[0].closed = true;
    latest.source_current = false;
    latest.closed = false;
    report.abstract_summaries.push(latest);
    report.validate().unwrap();
    let selection = Selection {
        state: Some(report.abstract_summaries[0].source_state),
        ..Default::default()
    };
    let context = context();
    let mut time = 0.0;
    let text = draw(&context, &mut time, &report, vec![])
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("historical observation · closed for observed input"));
    assert!(text.contains("open for observed input · final source uncertified"));
    let inspector = Inspector::default();
    assert!(
        inspector
            .accessible_summary(&(report).view(), selection)
            .contains("open for observed input · final source uncertified")
    );
    let latest = report.abstract_summaries.last_mut().unwrap();
    latest.source_current = true;
    latest.closed = true;
    let text = draw(&context, &mut time, &report, vec![])
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("historical observation · closed for observed input"));
    assert!(text.contains("closed for final source"));
    assert!(
        inspector
            .accessible_summary(&(report).view(), selection)
            .contains("closed for final source")
    );
}

#[test]
fn source_current_certified_observation_retains_actual_edges_deliveries_and_identity() {
    let mut report = observation_report(RECURSIVE);
    // Exercise the known-call presentation with real regional contents. The
    // server's native-to-wire integration separately verifies admission kinds.
    for summary in &mut report.abstract_summaries {
        summary.kind = api::ProcedureSummaryKind::KnownCall;
    }
    let cfg = &report.abstract_summaries[0].regional_cfg;
    let node = cfg
        .nodes
        .iter()
        .find(|node| {
            node.transfer.as_ref().is_some_and(|transfer| {
                transfer.current && transfer.complete && transfer.visited_block
            })
        })
        .unwrap();
    let context = context();
    let mut time = 0.0;
    click(&context, &mut time, &report, "Regional control flow");
    click(
        &context,
        &mut time,
        &report,
        &format!(
            "r{} · procedure {} · Current block transfer",
            node.id, node.procedure
        ),
    );
    click(&context, &mut time, &report, "Calls and returns");
    let visible = draw(&context, &mut time, &report, vec![]);
    let text = visible
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Known bytecode call"));
    assert!(!text.contains("Recursive bytecode"));
    assert!(text.contains(&format!(
        "Frame identity hash: {}",
        node.frame.code_hash.as_ref().unwrap()
    )));
    assert!(text.contains(&format!(
        "Storage namespace: {}",
        node.frame.storage_address.as_deref().unwrap()
    )));
    for edge in &node.transfer.as_ref().unwrap().successors {
        assert!(
            text.contains(&format!(
                "Current transfer: r{} → r{} · {:?}",
                node.id, edge.to, edge.kind
            )),
            "{text}"
        );
    }
    let mut delivered = 0;
    for call in &cfg.calls {
        let local_current = cfg.nodes[call.from]
            .transfer
            .as_ref()
            .is_some_and(|transfer| {
                transfer.current
                    && transfer.input_revision == cfg.nodes[call.from].input_revision
                    && !cfg.nodes[call.from].excluded
                    && transfer.calls.contains(&call.id)
            });
        let scope = if local_current {
            "current for final source"
        } else {
            "record observation"
        };
        assert!(text.contains(&format!(
            "Call c{} · {scope}: r{} → procedure {}",
            call.id, call.from, call.callee
        )));
        for delivery in &call.deliveries {
            let label = if delivery.current {
                "Current delivery"
            } else {
                "Historical delivery"
            };
            for edge in &delivery.successors {
                delivered += 1;
                assert!(
                    text.contains(&format!(
                        "{label}: r{} → r{} · {:?}",
                        call.from, edge.to, edge.kind
                    )),
                    "{text}"
                );
            }
        }
    }
    assert!(delivered > 0);
}

#[test]
fn source_current_does_not_restore_a_stale_local_receipt() {
    let mut report = observation_report(RECURSIVE);
    for node in &mut report.abstract_summaries[0].regional_cfg.nodes {
        node.input_revision.0 += 1;
        if let Some(transfer) = &mut node.transfer {
            transfer.current = false;
        }
    }
    for call in &mut report.abstract_summaries[0].regional_cfg.calls {
        call.input_revision.0 += 1;
        for delivery in &mut call.deliveries {
            delivery.current = false;
        }
    }
    report.validate().unwrap();
    let cfg = &report.abstract_summaries[0].regional_cfg;
    let node = cfg
        .nodes
        .iter()
        .find(|node| node.transfer.is_some())
        .unwrap();
    let context = context();
    let mut time = 0.0;
    click(&context, &mut time, &report, "Regional control flow");
    click(
        &context,
        &mut time,
        &report,
        &format!(
            "r{} · procedure {} · Historical transfer",
            node.id, node.procedure
        ),
    );
    click(&context, &mut time, &report, "Calls and returns");
    let visible = draw(&context, &mut time, &report, vec![]);
    let text = visible
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Historical transfer") && text.contains("Historical delivery"));
    assert!(!text.contains("Current transfer:") && !text.contains("Current delivery:"));
}

#[test]
fn regional_static_unknown_remains_an_effect_environment_abstraction_with_no_invented_pc() {
    let code = "5f5f5f5f6102015afa5000";
    // Keccak256 of the fixed input bytes, independent of the projected source.
    let expected_hash = "0x2e6963788ed56561b28953e622d7c6f0088d5c26c34186c3d4f9c8546eeea283";
    let root_owner = "0x1111111111111111111111111111111111111111";
    let report = observation_report(code);
    let summary = &report.abstract_summaries[0];
    let cfg = &summary.regional_cfg;
    let environment = summary.root_procedure.unwrap();
    let opaque = &summary.procedures[environment];
    assert_eq!(opaque.kind, api::CodeKind::AbstractEnvironment);
    assert_eq!(
        opaque.code_address.as_deref(),
        Some("0x0000000000000000000000000000000000000201")
    );
    assert!(opaque.is_static && opaque.code_hash.is_none() && opaque.covered_blocks.is_empty());
    let node = cfg
        .nodes
        .iter()
        .find(|node| {
            node.procedure == environment
                && node.frame.kind == api::CodeKind::AbstractEnvironment
                && node.transfer.as_ref().is_some_and(|transfer| {
                    transfer.current && transfer.complete && !transfer.visited_block
                })
        })
        .unwrap_or_else(|| panic!("expected current coarse environment receipt: {:?}", cfg));
    let transfer = node.transfer.as_ref().unwrap();
    assert!(transfer.current && transfer.complete && !transfer.visited_block);
    assert!(node.frame.basic_block.is_none() && node.frame.code_hash.is_none());
    assert!(node.frame.is_static);
    assert!(!transfer.calls.is_empty());
    // Unknown code stays opaque, while its real static callback can execute
    // the captured root. Keep their source and instruction evidence separate.
    let callback = summary
        .procedures
        .iter()
        .position(|procedure| procedure.kind == api::CodeKind::Runtime)
        .unwrap();
    assert_eq!(summary.procedures.len(), 2);
    let known = &summary.procedures[callback];
    assert!(known.is_static);
    assert_eq!(known.code_address.as_deref(), Some(root_owner));
    assert_eq!(known.storage_address.as_deref(), Some(root_owner));
    assert_eq!(known.code_hash.as_deref(), Some(expected_hash));
    assert_eq!(known.covered_blocks, [0, 1]);
    assert_eq!(opaque.dependencies, [callback]);
    assert_eq!(known.dependencies, [environment]);
    assert_ne!(known.code_address, opaque.code_address);
    let program = report
        .model
        .programs
        .iter()
        .find(|program| {
            program.kind == api::CodeKind::Runtime
                && program.code_address == known.code_address
                && Some(program.code_hash.as_str()) == known.code_hash.as_deref()
                && program.fork == report.fork
        })
        .unwrap();
    assert_eq!(program.bytecode.trim_start_matches("0x"), code);
    let source_blocks: std::collections::BTreeSet<_> =
        program.blocks.iter().map(|block| block.id).collect();
    assert_eq!(source_blocks, [0, 1].into_iter().collect());
    let mut visited = std::collections::BTreeSet::new();
    for node in &cfg.nodes {
        let transfer = node.transfer.as_ref().unwrap();
        assert!(transfer.current && transfer.complete && node.frame.is_static);
        if node.procedure == environment {
            assert_eq!(node.frame.kind, api::CodeKind::AbstractEnvironment);
            assert!(
                node.frame.basic_block.is_none()
                    && node.frame.code_hash.is_none()
                    && !transfer.visited_block
            );
        } else {
            assert_eq!(node.procedure, callback);
            assert_eq!(node.frame.kind, api::CodeKind::Runtime);
            assert_eq!(node.frame.code_address.as_deref(), Some(root_owner));
            assert_eq!(node.frame.storage_address.as_deref(), Some(root_owner));
            assert_eq!(node.frame.code_hash.as_deref(), Some(expected_hash));
            let block = node.frame.basic_block.unwrap();
            assert!(source_blocks.contains(&block) && transfer.visited_block);
            visited.insert(block);
        }
    }
    assert_eq!(visited, source_blocks);
    for id in &transfer.calls {
        let call = cfg.calls.iter().find(|call| call.id == *id).unwrap();
        assert_eq!(call.from, node.id);
        assert_eq!(call.callee, callback);
        assert!(!call.entries.is_empty() && !call.deliveries.is_empty());
        assert!(call.deliveries.iter().any(|delivery| delivery.current
            && delivery.kind == api::OutcomeKind::Return
            && delivery.input_revision == call.input_revision
            && delivery.output_generation > 0
            && !delivery.successors.is_empty()));
        assert!(call.entries.iter().all(|id| {
            cfg.nodes
                .iter()
                .any(|node| node.id == *id && node.procedure == callback)
        }));
    }
    assert!(cfg.calls.iter().any(|call| {
        call.callee == environment
            && cfg
                .nodes
                .iter()
                .any(|node| node.id == call.from && node.procedure == callback)
            && call
                .deliveries
                .iter()
                .any(|delivery| delivery.current && delivery.kind == api::OutcomeKind::Return)
    }));
    let unknown_states: Vec<_> = report
        .states
        .iter()
        .filter(|state| {
            state
                .entry
                .frames
                .last()
                .is_some_and(|frame| frame.kind == api::CodeKind::AbstractEnvironment)
        })
        .collect();
    assert!(!unknown_states.is_empty());
    for state in unknown_states {
        assert!(state.entry.frames.last().unwrap().program.is_none());
        let block = report
            .cfg
            .iter()
            .find(|block| block.id == state.state)
            .unwrap();
        assert!(block.source_block().is_none() && block.executed_pcs.is_empty());
    }
    let captured = cfg
        .nodes
        .iter()
        .find(|node| node.procedure == callback && node.frame.basic_block == Some(0))
        .unwrap();
    let captured_transfer = captured.transfer.as_ref().unwrap();
    assert_eq!(captured.frame.code_address, known.code_address);
    assert_eq!(captured.frame.code_hash, known.code_hash);
    assert!(
        captured.frame.is_static
            && captured_transfer.current
            && captured_transfer.complete
            && captured_transfer.visited_block
    );
    let context = context();
    let mut time = 0.0;
    click(&context, &mut time, &report, "Regional control flow");
    click(
        &context,
        &mut time,
        &report,
        &format!(
            "r{} · procedure {} · Current effect/environment abstraction",
            node.id, node.procedure
        ),
    );
    let visible = draw(&context, &mut time, &report, vec![]);
    let text = visible
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains(
        "callbacks may be represented by effects even when the retained call list is empty"
    ));
    assert!(
        !text.contains("Program block:")
            && !text.contains("Frame identity hash:")
            && !text.contains("pc 0x")
    );
    click(
        &context,
        &mut time,
        &report,
        &format!(
            "r{} · procedure {} · Current block transfer",
            captured.id, captured.procedure
        ),
    );
    let visible = draw(&context, &mut time, &report, vec![]);
    let text = visible
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Program block: 0")
            && text.contains("Frame identity hash:")
            && text.contains("captured block visited: true"),
        "{text}"
    );
}
