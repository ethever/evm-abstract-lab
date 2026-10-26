use super::Calls;
use crate::app::model::{
    ReportViewId,
    navigation::ContextFocus,
    presentation::{ContextTarget, Presentation, ReportIndex},
};
use evm_abstract_protocol::{AnalysisNamespace, CfgEdge, EdgeKind};

#[test]
fn calls_navigate_to_exact_child_source_owner_and_state_and_reject_stale_locations() {
    let mut report = crate::tests::report();
    let mut child = report.cfg[0].clone();
    child.id = 91;
    child.program = Some(7);
    child.code_address = Some("0x2222222222222222222222222222222222222222".into());
    child.storage_address = Some("0x3333333333333333333333333333333333333333".into());
    let source = crate::tests::source_program(
        7,
        child.code_address.clone(),
        report.model.programs[0].blocks.clone(),
    );
    report.model.programs.push(source);
    report.cfg.push(child);
    report.edges.push(CfgEdge {
        id: 900,
        from: 0,
        to: 91,
        kind: EdgeKind::Call,
    });
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = presentation
        .focus_context(ContextTarget {
            view,
            context: ContextFocus::Graph { state: 0 },
        })
        .unwrap();
    let mut calls = Calls::default();
    calls.prepare(presentation, &focus);
    assert_eq!(calls.links.len(), 1);
    let target = &calls.links[0];
    assert_eq!(
        target.focus.context,
        Some(ContextFocus::Graph { state: 91 })
    );
    assert_eq!(
        target
            .focus
            .source
            .as_ref()
            .and_then(|source| source.program()),
        Some(7)
    );
    assert!(
        target
            .detail
            .contains("0x3333333333333333333333333333333333333333")
    );
    assert!(presentation.validate(&target.focus).is_ok());
    let mut stale = focus;
    stale.view = ReportViewId::fixture(2, AnalysisNamespace::Native);
    calls.prepare(presentation, &stale);
    assert!(calls.links.is_empty());
}

#[test]
fn semantic_source_labels_use_the_recorded_source_before_a_fallback_address() {
    use evm_abstract_protocol::{
        CodeKind, Fork, ProcessForkSource, ProcessPrecompileSource, ProcessSource,
    };
    let precompile = ProcessSource::Precompile(ProcessPrecompileSource {
        address: format!("0x{:040x}", 3),
        fork: Fork::Osaka,
    });
    let proxy = format!("0x{:040x}", 0x202);
    assert_eq!(
        super::source_label(Some(&precompile), Some(CodeKind::Runtime), &proxy),
        "Precompile 3"
    );
    assert_eq!(
        super::source_name(Some(&precompile)),
        format!("0x{:040x}", 3)
    );
    let unknown = ProcessSource::AnyRuntime(ProcessForkSource { fork: Fork::Osaka });
    assert_eq!(
        super::source_label(Some(&unknown), Some(CodeKind::Runtime), &proxy),
        "Unknown external"
    );
}

fn alias_calls() -> Calls {
    let mut report = crate::tests::report();
    for (offset, code, owner, program) in [
        (0, 0x101, 0x3101, 7),
        (1, 0x201, 0x3101, 8),
        (2, 0x202, 0x3101, 9),
        (3, 0x202, 0x3102, 9),
    ] {
        let address = format!("0x{code:040x}");
        if !report
            .model
            .programs
            .iter()
            .any(|source| source.id == program)
        {
            let source = crate::tests::source_program(
                program,
                Some(address.clone()),
                report.model.programs[0].blocks.clone(),
            );
            report.model.programs.push(source);
        }
        let mut child = report.cfg[0].clone();
        child.id = 91 + offset;
        child.program = Some(program);
        child.code_address = Some(address);
        child.storage_address = Some(format!("0x{owner:040x}"));
        report.cfg.push(child);
        report.edges.push(CfgEdge {
            id: 900 + offset,
            from: 0,
            to: 91 + offset,
            kind: EdgeKind::Call,
        });
    }
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = presentation
        .focus_context(ContextTarget {
            view,
            context: ContextFocus::Graph { state: 0 },
        })
        .unwrap();
    let mut calls = Calls::default();
    calls.prepare(presentation, &focus);
    assert_eq!(calls.links.len(), 4);
    calls
}

fn render(context: &egui::Context, calls: &Calls, events: Vec<egui::Event>) -> egui::FullOutput {
    render_with_actions(context, calls, events).0
}

fn render_with_actions(
    context: &egui::Context,
    calls: &Calls,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Vec<crate::app::model::navigation::Focus>) {
    let mut actions = Vec::new();
    let mut output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(200.0, 640.0),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            // A 200-point Sources panel loses space to padding, indentation and its
            // scrollbar. Test the same row renderer with only 160 points for content.
            ui.set_width(160.0);
            for (index, link) in calls.links.iter().enumerate() {
                ui.push_id(index, |ui| {
                    if let Some(super::Action::Navigate(
                        crate::app::model::navigation::Intent::Open(focus),
                    )) = super::show_link(ui, link)
                    {
                        actions.push(focus);
                    }
                });
            }
        },
    );
    output.textures_delta.clear();
    (output, actions)
}

fn text_rect(output: &egui::FullOutput, predicate: impl Fn(&str) -> bool) -> egui::Rect {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if predicate(text.galley.text()) => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .expect("painted label")
}

#[test]
fn compact_call_rows_paint_distinct_runtime_and_owner_suffixes_without_hover() {
    let calls = alias_calls();
    let context = egui::Context::default();
    crate::palette::configure(&context);
    render(&context, &calls, vec![]);
    let output = render(&context, &calls, vec![]);
    for suffix in ["00000101", "00000201", "00000202"] {
        let rect = text_rect(&output, |text| {
            !text.starts_with("Owner") && text.ends_with(suffix)
        });
        assert!(
            rect.right() <= 160.0,
            "target suffix must be painted within the compact row: {suffix} {rect:?}"
        );
    }
    for suffix in ["00003101", "00003102"] {
        let rect = text_rect(&output, |text| {
            text.starts_with("Owner") && text.ends_with(suffix)
        });
        assert!(
            rect.right() <= 160.0,
            "owner suffix must remain visible: {suffix} {rect:?}"
        );
    }
    assert_eq!(calls.links[2].title, calls.links[3].title);
    assert_ne!(calls.links[2].owner, calls.links[3].owner);
    assert_ne!(calls.links[2].focus.context, calls.links[3].focus.context);
}

fn click(context: &egui::Context, calls: &Calls, pos: egui::Pos2) -> egui::FullOutput {
    for pressed in [true, false] {
        let output = render(
            context,
            calls,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        if !pressed {
            return output;
        }
    }
    unreachable!()
}

#[test]
fn primary_click_opens_selectable_full_identity_and_copies_the_exact_target() {
    let calls = alias_calls();
    let context = egui::Context::default();
    crate::palette::configure(&context);
    render(&context, &calls, vec![]);
    let output = render(&context, &calls, vec![]);
    let details = text_rect(&output, |text| text.starts_with("Owner")).center();
    click(&context, &calls, details);
    let output = render(&context, &calls, vec![]);
    let text = crate::tests::painted_text(&output)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains(&format!("Code: 0x{:040x}", 0x101)), "{text}");
    assert!(
        text.contains(&format!("Owner: 0x{:040x}", 0x3101)),
        "{text}"
    );
    let copy = text_rect(&output, |text| text == "Copy full target").center();
    let output = click(&context, &calls, copy);
    assert!(
        output
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command,
        egui::OutputCommand::CopyText(text) if text == &calls.links[0].detail))
    );
}

#[test]
fn choosing_the_same_code_with_another_owner_opens_that_exact_context() {
    let calls = alias_calls();
    let context = egui::Context::default();
    crate::palette::configure(&context);
    render(&context, &calls, vec![]);
    let output = render(&context, &calls, vec![]);
    let pos = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text)
                if !text.galley.text().starts_with("Owner")
                    && text.galley.text().ends_with("00000202") =>
            {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .next_back()
        .unwrap();
    render_with_actions(
        &context,
        &calls,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let (_, actions) = render_with_actions(
        &context,
        &calls,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert_eq!(actions, vec![calls.links[3].focus.clone()]);
    assert_eq!(actions[0].context, Some(ContextFocus::Graph { state: 94 }));
    assert_eq!(
        actions[0]
            .source
            .as_ref()
            .and_then(|source| source.program()),
        Some(9)
    );
}
