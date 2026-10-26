use crate::app::model::navigation::{Focus, Intent, NavigationError};
use crate::app::{Selection, View};

#[test]
fn invalid_legacy_selection_restores_the_canonical_context_without_history() {
    let mut workspace = crate::tests::ready();
    let current = workspace.report_view.navigator().current().unwrap().clone();
    let selected = workspace.selection;
    let camera = workspace.graph.camera();
    workspace.selection = Selection {
        state: Some(usize::MAX),
        pc: Some(0),
        value: None,
    };
    workspace.reconcile_widget_selection();
    assert_eq!(workspace.selection, selected);
    assert_eq!(workspace.report_view.navigator().current(), Some(&current));
    assert_eq!(workspace.graph.camera(), camera);
    assert_eq!(
        workspace.navigation_error,
        Some(NavigationError::MissingContext)
    );
}

#[test]
fn stale_open_cannot_change_route_camera_or_the_replacement_report() {
    let mut workspace = crate::tests::ready();
    let stale = workspace
        .report_view
        .navigator()
        .current()
        .unwrap()
        .focus
        .clone();
    workspace.receive(Ok(evm_abstract_protocol::AnalyzeReply {
        result: Ok(crate::tests::report().into()),
    }));
    let current = workspace.report_view.navigator().current().unwrap().clone();
    let selected = workspace.selection;
    let camera = workspace.graph.camera();
    let route = workspace.view;
    workspace.navigate(Intent::Open(stale), Some(View::Disassembly));
    assert_eq!(
        workspace.navigation_error,
        Some(NavigationError::StaleReport)
    );
    assert_eq!(workspace.report_view.navigator().current(), Some(&current));
    assert_eq!(workspace.selection, selected);
    assert_eq!(workspace.graph.camera(), camera);
    assert_eq!(workspace.view, route);
}

#[test]
fn back_and_forward_restore_each_routes_actual_camera_and_exact_native_selection() {
    let mut workspace = crate::tests::ready();
    workspace.view = View::Disassembly;
    let root = workspace.selection;
    let root_camera = (1.25, egui::vec2(123.0, 44.0));
    workspace.graph.restore_camera(root_camera.0, root_camera.1);
    let target = Selection {
        state: Some(2),
        pc: None,
        value: None,
    };
    let focus = workspace
        .report_view
        .presentation(workspace.report.as_ref().unwrap())
        .graph_focus(workspace.report_view.identity(), target, None)
        .unwrap();
    workspace.navigate(Intent::Open(focus), Some(View::Graph));
    let child_camera = (0.7, egui::vec2(55.0, -11.0));
    workspace
        .graph
        .restore_camera(child_camera.0, child_camera.1);
    workspace.navigate(Intent::Back, None);
    assert_eq!(workspace.view, View::Disassembly);
    assert_eq!(workspace.selection, root);
    assert_eq!(workspace.graph.camera(), root_camera);
    workspace.navigate(Intent::Forward, None);
    assert_eq!(workspace.view, View::Graph);
    assert_eq!(workspace.selection, target);
    assert_eq!(workspace.graph.camera(), child_camera);
}

#[test]
fn source_only_original_inspection_never_creates_a_native_or_procedure_selection() {
    let mut workspace = crate::tests::ready();
    let source = workspace
        .report_view
        .presentation(workspace.report.as_ref().unwrap())
        .source(0)
        .unwrap();
    workspace.navigate(
        Intent::Open(Focus::source(workspace.report_view.identity(), source)),
        Some(View::Disassembly),
    );
    assert_eq!(workspace.selection, Selection::default());
    assert!(
        workspace
            .report_view
            .navigator()
            .current()
            .unwrap()
            .focus
            .context
            .is_none()
    );
    workspace.inspect_original(crate::app::workbench::inspection::OriginalSubject::Root);
    let context = egui::Context::default();
    crate::palette::configure(&context);
    let mut output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 900.0),
            )),
            ..Default::default()
        },
        |ui| {
            workspace.show(ui);
        },
    );
    output.textures_delta.clear();
    assert_eq!(workspace.selection, Selection::default());
    assert!(
        workspace
            .report_view
            .navigator()
            .current()
            .unwrap()
            .focus
            .context
            .is_none()
    );
    assert!(
        workspace
            .accessible_status()
            .contains("Original observations")
    );
}
