//! Producer reports drive scope/availability tests; a separate display-only
//! counterexample gives joint alternatives distinct values to detect mixing.

use super::*;
use crate::app::minimum;
use std::sync::OnceLock;

mod original;

fn report() -> &'static api::AnalysisReport {
    static REPORT: OnceLock<api::AnalysisReport> = OnceLock::new();
    REPORT.get_or_init(|| {
        // Unknown calldata chooses two exits with different data and writes.
        let report = evm_abstract_server::analyze::analyze(api::AnalyzeRequest {
            input: api::AnalysisInput::Bytecode(api::BytecodeInput {
                bytecode: "5f351560125760aa5f5560115f5260205ff35b60bb5f5560225f5260205ffd".into(),
                address: None,
            }),
            limits: api::AnalysisLimits {
                root_terminal_coverage: false,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        report.validate().unwrap();
        report
    })
}

fn selection(model: &api::MinimumModel) -> minimum::Selection {
    let mut selection = minimum::Selection::default();
    selection.select_summary(model, minimum::initial_summary(model));
    selection
}

fn render(
    ctx: &egui::Context,
    time: &mut f64,
    inspector: &mut Inspector,
    selected: minimum::Selection,
    events: Vec<egui::Event>,
) -> (Vec<(String, egui::Rect)>, Option<InspectionAction>) {
    render_report(report(), ctx, time, inspector, selected, events)
}

fn render_report(
    report: &api::AnalysisReport,
    ctx: &egui::Context,
    time: &mut f64,
    inspector: &mut Inspector,
    selected: minimum::Selection,
    events: Vec<egui::Event>,
) -> (Vec<(String, egui::Rect)>, Option<InspectionAction>) {
    let mut action = None;
    let text = rendered_at(ctx, time, events, egui::vec2(1200.0, 1800.0), |ui| {
        action = inspector.inspect(
            ui,
            InspectionTarget::Global {
                report,
                model: report.minimum.as_deref().unwrap(),
                selection: selected,
            },
        );
    });
    (text, action)
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::palette::configure(&ctx);
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    ctx
}

#[test]
fn rendering_original_accounts_does_not_change_the_shell_selection() {
    let ctx = context();
    let mut time = 0.0;
    let mut inspector = Inspector::default();
    let selected = selection(report().minimum.as_deref().unwrap());
    let (text, action) = render(&ctx, &mut time, &mut inspector, selected, vec![]);
    assert_eq!(action, None);
    let position = text
        .iter()
        .find(|(label, _)| label == "Original accounts and root template")
        .unwrap()
        .1
        .center();
    for pressed in [true, false] {
        let (_, action) = render(
            &ctx,
            &mut time,
            &mut inspector,
            selected,
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
        assert_eq!(action, None);
    }
    let (text, action) = render(&ctx, &mut time, &mut inspector, selected, vec![]);
    assert!(
        text.iter()
            .any(|(text, _)| text.contains("Root input template")),
        "{text:?}"
    );
    assert_eq!(action, None);
    assert_eq!(selected.account, None);
}

#[test]
fn selected_outcome_keeps_its_return_data_and_store_together() {
    let mut report = report().clone();
    // Conservative outputs may share identical unknown numerical bounds.
    // These distinct typed facts are a UI counterexample for selector mixing,
    // not execution evidence or a precision claim about this program.
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let selected = selection(model);
    let bounds = &mut std::sync::Arc::make_mut(&mut model.summaries[selected.summary].bounds)
        [selected.procedure];
    for output in &mut bounds.outputs {
        let (data, store) = match output.kind {
            api::OutcomeKind::Return => ("0x11", "0xaa"),
            api::OutcomeKind::Revert => ("0x22", "0xbb"),
            _ => continue,
        };
        output.data = api::MinimumByteArray {
            length: word("0x1"),
            memory: false,
            content: api::MinimumByteContent::Exact(data.into()),
            words: vec![],
        };
        output.store.persistent.default = word(store);
        output.store.persistent.slots.clear();
        output.store.persistent.owners.clear();
    }
    model.validate().unwrap();
    let model = report.minimum.as_deref().unwrap();
    let selected = selection(model);
    let bounds = &selected.summary(model).unwrap().bounds[selected.procedure];
    let returned = bounds
        .outputs
        .iter()
        .position(|output| output.kind == api::OutcomeKind::Return)
        .expect("actual RETURN output");
    let reverted = bounds
        .outputs
        .iter()
        .position(|output| output.kind == api::OutcomeKind::Revert)
        .expect("actual REVERT output");
    assert_ne!(bounds.outputs[returned].data, bounds.outputs[reverted].data);
    assert_ne!(
        bounds.outputs[returned].store,
        bounds.outputs[reverted].store
    );
    let ctx = context();
    let mut time = 0.0;
    let mut inspector = Inspector::default();
    for (chosen, other, byte) in [(returned, reverted, "11"), (reverted, returned, "22")] {
        let selected = minimum::Selection {
            output: Some(chosen),
            ..selected
        };
        inspector.tab = Tab::Returndata;
        let (data, action) =
            render_report(&report, &ctx, &mut time, &mut inspector, selected, vec![]);
        assert_eq!(action, None);
        assert_eq!(inspector.scope, super::super::minimum::Scope::Outcome);
        assert!(
            data.iter().any(|(text, _)| text.ends_with(byte)),
            "{data:?}"
        );
        inspector.tab = Tab::Storage;
        let (store, action) =
            render_report(&report, &ctx, &mut time, &mut inspector, selected, vec![]);
        assert_eq!(action, None);
        let value = &bounds.outputs[chosen].store.persistent.default.summary;
        let other = &bounds.outputs[other].store.persistent.default.summary;
        assert!(
            store.iter().any(|(text, _)| text == value),
            "missing {value}: {store:?}"
        );
        assert!(
            !store.iter().any(|(text, _)| text == other),
            "a different outcome's store leaked: {store:?}"
        );
    }
}

#[test]
fn unavailable_context_node_and_output_are_rejected_without_clamping_or_selection_changes() {
    let model = report().minimum.as_deref().unwrap();
    let selected = selection(model);
    let ctx = context();
    let mut time = 0.0;
    for (selected, expected) in [
        (
            minimum::Selection {
                procedure: usize::MAX,
                ..selected
            },
            "This context is unavailable",
        ),
        (
            minimum::Selection {
                node: Some(usize::MAX),
                ..selected
            },
            "does not belong to this context",
        ),
        (
            minimum::Selection {
                output: Some(usize::MAX),
                ..selected
            },
            "This outcome is unavailable",
        ),
        (
            minimum::Selection {
                control: Some(api::ProcessLocation::Block(usize::MAX)),
                ..selected
            },
            "not attached to this regional observation",
        ),
    ] {
        let mut inspector = Inspector {
            scope: super::super::minimum::Scope::Input,
            ..Default::default()
        };
        let (text, action) = render(&ctx, &mut time, &mut inspector, selected, vec![]);
        assert_eq!(action, None);
        assert!(
            text.iter().any(|(text, _)| text.contains(expected)),
            "{text:?}"
        );
        assert!(
            !text.iter().any(
                |(text, _)| text == "Joined procedure input bounds" || text == "Joined ADDRESS"
            )
        );
    }
}

#[test]
fn same_local_id_in_global_cannot_borrow_the_native_root_address_condition() {
    let report: crate::tests::AnalysisReport = report().clone().into();
    let ctx = context();
    let mut time = 0.0;
    let mut inspector = Inspector::default();
    for namespace in [
        api::AnalysisNamespace::Native,
        api::AnalysisNamespace::Baseline,
    ] {
        let graph = report.graph(namespace).unwrap();
        let selection = Selection {
            state: Some(graph.cfg[0].id),
            ..Default::default()
        };
        let mut action = None;
        let text = rendered_at(&ctx, &mut time, vec![], egui::vec2(1200.0, 1600.0), |ui| {
            action = inspector.inspect(
                ui,
                InspectionTarget::Native {
                    report: &graph,
                    selection,
                    account: None,
                },
            );
        });
        assert_eq!(action, None);
        assert_eq!(
            text.iter()
                .any(|(text, _)| text.starts_with("Source condition · root ADDRESS =")),
            namespace == api::AnalysisNamespace::Native,
            "{text:?}"
        );
    }
}

#[test]
fn source_only_and_global_may_cannot_inherit_native_stack_memory_or_ssa() {
    let report = report();
    let model = report.minimum.as_deref().unwrap();
    let selected = selection(model);
    let ctx = context();
    let mut time = 0.0;
    for tab in [Tab::Stack, Tab::Memory, Tab::SsaValue] {
        let mut inspector = Inspector {
            tab,
            scope: super::super::minimum::Scope::Input,
            ..Default::default()
        };
        let (text, action) = render(&ctx, &mut time, &mut inspector, selected, vec![]);
        assert_eq!(action, None);
        assert!(
            text.iter().any(|(text, _)| text.contains(match tab {
                Tab::Stack => "no Native instruction stack",
                Tab::Memory => "no Native memory snapshot",
                _ => "no Native SSA definition",
            })),
            "{text:?}"
        );
        let mut action = None;
        let text = rendered_at(&ctx, &mut time, vec![], egui::vec2(1200.0, 1200.0), |ui| {
            action = inspector.inspect(
                ui,
                InspectionTarget::Source {
                    report,
                    source: InspectionSource::Program(report.programs[0].id),
                },
            );
        });
        assert_eq!(action, None);
        assert!(
            text.iter()
                .any(|(text, _)| text.contains("no execution context selected"))
        );
        assert!(
            text.iter()
                .any(|(text, _)| text.contains("needs an explicit observation"))
        );
        assert!(
            !text
                .iter()
                .any(|(text, _)| text.contains("Joined procedure input bounds")
                    || text.contains("Paired path conditions"))
        );
    }
}
