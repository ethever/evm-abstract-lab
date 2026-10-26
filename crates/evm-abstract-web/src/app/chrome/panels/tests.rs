use crate::{Workspace, app::View};
use egui::{Context, Event, FullOutput, PointerButton, Pos2, RawInput, Rect};

fn frame(context: &Context, workspace: &mut Workspace, events: Vec<Event>) -> FullOutput {
    crate::controls::begin_observation(context, true);
    let mut output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 480.0))),
            events,
            ..RawInput::default()
        },
        |ui| {
            workspace.show(ui);
        },
    );
    output.textures_delta.clear();
    output
}

fn click(context: &Context, workspace: &mut Workspace, label: &str) {
    let output = frame(context, workspace, vec![]);
    let position = crate::tests::painted_text(&output)
        .into_iter()
        .find(|(text, _)| text == label)
        .unwrap_or_else(|| panic!("visible {label} action"))
        .1
        + egui::vec2(10.0, 6.0);
    for pressed in [true, false] {
        frame(
            context,
            workspace,
            vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn phone_result_opens_cfg_and_keeps_panel_actions_above_the_workspace() {
    let context = Context::default();
    crate::palette::configure(&context);
    let mut workspace = Workspace::default();
    workspace.initial_command();
    workspace.receive(Ok(evm_abstract_protocol::AnalyzeReply {
        result: Ok(crate::tests::report().into()),
    }));
    frame(&context, &mut workspace, vec![]);
    let output = frame(&context, &mut workspace, vec![]);
    assert_eq!(workspace.view, View::Graph);
    assert!(!workspace.directory.open && !workspace.inspector.expanded);
    for label in ["Sources", "Inspect", "Input"] {
        let (_, position) = crate::tests::painted_text(&output)
            .into_iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("{label} remains reachable on the phone"));
        assert!(position.y >= 0.0 && position.y <= 40.0);
        assert!(position.x >= 0.0 && position.x <= 300.0);
        let controls: Vec<serde_json::Value> = serde_json::from_str(
            &crate::controls::observed(&context).expect("requested actual control geometry"),
        )
        .unwrap();
        let control = controls
            .iter()
            .find(|control| control["label"] == label)
            .unwrap();
        let rect = control["rect"].as_array().unwrap();
        assert!(rect[3].as_f64().unwrap() - rect[1].as_f64().unwrap() >= 32.0);
        assert_eq!(control["enabled"], true);
    }
    click(&context, &mut workspace, "Sources");
    assert!(workspace.directory.open && !workspace.inspector.expanded);
    click(&context, &mut workspace, "Inspect");
    assert!(!workspace.directory.open && workspace.inspector.expanded);
    // Ordinary repaints retain the user's explicit choice.
    frame(&context, &mut workspace, vec![]);
    assert!(workspace.inspector.expanded);
}
