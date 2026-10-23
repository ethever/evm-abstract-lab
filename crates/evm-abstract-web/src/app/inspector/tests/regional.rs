//! Rendering consumes actual regional receipts, with no extra instruction/SSA data.
use super::*;

const RECURSIVE: &str = "5f5f5f5f5f305af150602a5f5260205ff3";

fn observation_report(bytecode: &str) -> api::AnalysisReport {
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
            evidence::diagnostics(ui, &report, &mut selection);
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
    report: &api::AnalysisReport,
    events: Vec<egui::Event>,
) -> Vec<(String, egui::Rect)> {
    let source = report.abstract_summaries[0].source_state;
    rendered_at(context, time, events, egui::vec2(1800.0, 16000.0), |ui| {
        summaries::show(ui, report, Some(source));
    })
}

fn click(context: &egui::Context, time: &mut f64, report: &api::AnalysisReport, label: &str) {
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
    let top = crate::tests::empty_report().metadata.environment.call_value;
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
    let accessible = Inspector::default().accessible_summary(&report, selection);
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
            .accessible_summary(&report, selection)
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
            .accessible_summary(&report, selection)
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
    let report = observation_report("5f5f5f5f6102015afa5000");
    let cfg = &report.abstract_summaries[0].regional_cfg;
    let node = cfg
        .nodes
        .iter()
        .find(|node| {
            node.frame.kind == api::CodeKind::AbstractEnvironment
                && node.transfer.as_ref().is_some_and(|transfer| {
                    transfer.current
                        && transfer.complete
                        && !transfer.visited_block
                        && transfer.calls.is_empty()
                })
        })
        .unwrap_or_else(|| panic!("expected current coarse environment receipt: {:?}", cfg));
    let transfer = node.transfer.as_ref().unwrap();
    assert!(
        transfer.current
            && transfer.complete
            && !transfer.visited_block
            && transfer.calls.is_empty()
    );
    assert!(node.frame.basic_block.is_none() && node.frame.code_hash.is_none());
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
}
