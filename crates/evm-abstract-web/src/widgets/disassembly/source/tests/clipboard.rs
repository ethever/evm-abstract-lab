use super::*;

fn copied(output: &egui::FullOutput) -> Vec<String> {
    output
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn click(
    context: &Context,
    cache: &mut SourceCache,
    presentation: Presentation<'_>,
    focus: &Focus,
    pos: egui::Pos2,
    button: egui::PointerButton,
) -> egui::FullOutput {
    render(
        context,
        cache,
        presentation,
        focus,
        egui::vec2(900.0, 500.0),
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    render(
        context,
        cache,
        presentation,
        focus,
        egui::vec2(900.0, 500.0),
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    )
    .0
}

#[test]
fn production_copy_actions_keep_full_code_bytes_rows_and_qualified_pc_reference() {
    let mut report = crate::tests::report();
    let source = std::sync::Arc::make_mut(&mut report.model.programs[0]);
    let immediate = format!("0x{}", "fe".repeat(32));
    source.blocks[0].instructions[0].name = "PUSH32".into();
    source.blocks[0].instructions[0].opcode = 0x7f;
    source.blocks[0].instructions[0].immediate = Some(immediate.clone());
    source.bytecode = format!("0x7f{}00", "fe".repeat(32));
    let row = super::super::super::copy::instruction(&source.blocks[0].instructions[0]);
    let expected_code = super::super::super::copy::code(source);
    let expected_bytes = source.bytecode.clone();
    let hash = source.code_hash.clone();
    let view = ReportViewId::fixture(17, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = Focus::source(view, presentation.source(0).unwrap());
    let context = Context::default();
    crate::palette::configure(&context);
    let mut cache = SourceCache::default();
    render(
        &context,
        &mut cache,
        presentation,
        &focus,
        egui::vec2(900.0, 500.0),
        vec![],
    );
    for (button, expected) in [
        ("Copy code", expected_code),
        ("Copy bytecode", expected_bytes),
    ] {
        let (output, _) = render(
            &context,
            &mut cache,
            presentation,
            &focus,
            egui::vec2(900.0, 500.0),
            vec![],
        );
        let pos = text_rect(&output, button).center();
        let output = click(
            &context,
            &mut cache,
            presentation,
            &focus,
            pos,
            egui::PointerButton::Primary,
        );
        assert_eq!(copied(&output), vec![expected]);
    }
    for action in ["Copy row", "Copy source / PC reference"] {
        let (output, _) = render(
            &context,
            &mut cache,
            presentation,
            &focus,
            egui::vec2(900.0, 500.0),
            vec![],
        );
        let pos = text_rect(&output, &row).center();
        click(
            &context,
            &mut cache,
            presentation,
            &focus,
            pos,
            egui::PointerButton::Secondary,
        );
        let (output, _) = render(
            &context,
            &mut cache,
            presentation,
            &focus,
            egui::vec2(900.0, 500.0),
            vec![],
        );
        let pos = text_rect(&output, action).center();
        let output = click(
            &context,
            &mut cache,
            presentation,
            &focus,
            pos,
            egui::PointerButton::Primary,
        );
        let payload = copied(&output);
        assert_eq!(payload.len(), 1);
        if action == "Copy row" {
            assert_eq!(payload[0], row);
            assert!(payload[0].contains(&immediate));
        } else {
            for expected in [
                hash.as_str(),
                "PC: 0x0",
                "Block: 0",
                "Native",
                "17",
                "Publication:",
                "Context: None",
            ] {
                assert!(
                    payload[0].contains(expected),
                    "missing {expected}: {}",
                    payload[0]
                );
            }
            assert!(
                !payload[0].contains("State:"),
                "a source-only copy must not add an execution state"
            );
        }
    }
}
