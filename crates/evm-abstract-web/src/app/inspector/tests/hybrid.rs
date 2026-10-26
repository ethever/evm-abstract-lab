//! Real default reports drive both graph scopes and process callback/creation UI.
use super::*;

fn context() -> egui::Context {
    let context = egui::Context::default();
    crate::palette::configure(&context);
    context
}

fn report(code: &str) -> crate::tests::AnalysisReport {
    let request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: code.into(),
            address: None,
        }),
        limits: api::AnalysisLimits {
            root_terminal_coverage: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    (report).into()
}

#[test]
fn default_coverage_panel_shows_conditioned_native_paths_without_replacing_global_routes() {
    let report = report("600160020100");
    let global = report.graph(api::AnalysisNamespace::Baseline).unwrap();
    assert!(!global.abstract_summaries.is_empty());
    let native = report.graph(api::AnalysisNamespace::Native).unwrap();
    assert!(!native.cfg.is_empty());
    let context = context();
    let mut time = 0.0;
    let mut inspector = Inspector {
        tab: Tab::Coverage,
        ..Default::default()
    };
    let mut selection = Selection::default();
    let visible = rendered_at(
        &context,
        &mut time,
        vec![],
        egui::vec2(1600.0, 2000.0),
        |ui| {
            inspector.show(ui, &(report).view(), &mut selection, None);
        },
    );
    let text = visible
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "Global coverage closed",
        "Global model",
        "Detailed paths",
        "Additional condition: root ADDRESS",
        "dispatch entered",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert_eq!(
        global.abstract_summaries.len(),
        report.abstract_summaries.len()
    );
    let mut inspector = Inspector::default();
    let mut selection = Selection {
        state: global.cfg.first().map(|node| node.id),
        ..Default::default()
    };
    let visible = rendered_at(
        &context,
        &mut time,
        vec![],
        egui::vec2(1600.0, 2000.0),
        |ui| inspector.show(ui, &global, &mut selection, None),
    );
    assert!(
        visible
            .iter()
            .any(|(text, _)| text.contains("Original input entry"))
    );
    assert!(
        visible
            .iter()
            .any(|(text, _)| text == "Original input snapshot")
    );
    assert!(
        inspector
            .accessible_summary(&global, selection)
            .contains("Original input entry")
    );
    assert!(!super::super::summaries::original_input(
        &native,
        Some(native.cfg[0].id)
    ));
    report.validate().unwrap();
}

#[test]
fn retained_precision_sources_are_shown_without_relabeling_baseline_closure() {
    let mut report = report("600160020100");
    let hybrid = report.model.hybrid.as_mut().unwrap();
    let mut precision = hybrid.coverage.clone();
    assert_eq!(precision.detail_regions.len(), 1);
    assert_eq!(
        precision.requested_detail_regions,
        hybrid.coverage.requested_detail_regions
    );
    // Display-only counterexample over the existing report fixture. It models
    // distinct metadata lists, not RPC acquisition, admission or solving.
    precision.detail_current = false;
    hybrid.coverage.detail_regions.clear();
    hybrid.precision = Some(precision);
    let expected = "Detailed code sources: 1 selected · limit 1";
    assert_eq!(
        crate::app::hybrid::source_policy(&report.model).as_deref(),
        Some(expected)
    );
    let summary = crate::app::hybrid::summary(&report.model).unwrap();
    assert!(summary.contains("Global coverage closed"));
    assert!(
        summary.contains("detailed paths pending") && summary.contains("root address condition")
    );
    let context = context();
    let mut time = 0.0;
    let mut inspector = Inspector {
        tab: Tab::Coverage,
        ..Default::default()
    };
    let mut selection = Selection::default();
    let visible = rendered_at(
        &context,
        &mut time,
        vec![],
        egui::vec2(1600.0, 2000.0),
        |ui| {
            inspector.show(ui, &report.view(), &mut selection, None);
        },
    );
    assert!(visible.iter().any(|(text, _)| text.contains(expected)));
    let hybrid = report.model.hybrid.as_mut().unwrap();
    assert!(hybrid.native_coverage().baseline_current);
    hybrid.coverage.baseline_current = false;
    let summary = crate::app::hybrid::summary(&report.model).unwrap();
    assert!(summary.contains("Global coverage incomplete") && summary.contains(expected));
}

#[test]
fn actual_static_callbacks_and_constructor_outcomes_render_without_process_pcs() {
    for (code, constructor) in [("5f5f5f5f6102015afa5000", false), ("5f5f5ff000", true)] {
        let report = report(code);
        let procedures = report
            .abstract_summaries
            .iter()
            .flat_map(|summary| &summary.procedures);
        let key = procedures
            .filter_map(|procedure| procedure.process.as_ref())
            .find(|key| {
                if constructor {
                    key.source.semantics() == api::TransferSemantics::ConstructorProcess
                } else {
                    key.is_static && key.source.kind() == api::CodeKind::Runtime
                }
            })
            .expect("actual process constructor or static callback must remain represented");
        let cases = report
            .abstract_summaries
            .iter()
            .flat_map(|summary| &summary.regional_cfg.calls)
            .flat_map(|call| &call.deliveries)
            .flat_map(|delivery| &delivery.creation)
            .collect::<Vec<_>>();
        let context = context();
        let mut time = 0.0;
        let visible = rendered_at(
            &context,
            &mut time,
            vec![],
            egui::vec2(1800.0, 4000.0),
            |ui| {
                process::frame(ui, key);
                for case in &cases {
                    process::completion(ui, case);
                }
            },
        );
        let text = visible
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !text.contains("Program block:") && !text.contains("PC 0x"),
            "{text}"
        );
        if constructor {
            assert!(!cases.is_empty());
            assert!(
                text.contains("Constructor process · before runtime deployment"),
                "{text}"
            );
            assert!(
                text.contains("Creation result: zero") && text.contains("Created address class:"),
                "{text}"
            );
        } else {
            assert!(
                text.contains("Static: true") && text.contains("possible control"),
                "{text}"
            );
            assert!(
                report
                    .abstract_summaries
                    .iter()
                    .flat_map(|summary| &summary.regional_cfg.calls)
                    .any(|call| call.action == Some(api::ProcessAction::StaticCall)
                        && call.scope == Some(api::ProcessControlScope::Primitive))
            );
        }
    }
}
