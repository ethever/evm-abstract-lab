use super::*;

#[test]
fn child_source_keeps_unexecuted_rows_and_emits_only_its_actual_source_coordinate() {
    let mut report = crate::tests::report();
    let mut first = report.model.programs[0].blocks[0].clone();
    first.instructions.truncate(1);
    first.instructions[0].name = "CALLDATASIZE".into();
    first.instructions[0].opcode = 0x36;
    let mut unexecuted = first.clone();
    unexecuted.id = 9;
    unexecuted.start_pc = 88;
    unexecuted.instructions[0].pc = 88;
    unexecuted.instructions[0].name = "RETURN".into();
    unexecuted.instructions[0].opcode = 0xf3;
    report.model.programs.push(crate::tests::source_program(
        7,
        Some(format!("0x{:040x}", 0x202)),
        vec![first, unexecuted],
    ));
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = Focus::source(view, presentation.source(7).unwrap());
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    let mut cache = SourceCache::default();
    let size = egui::vec2(700.0, 400.0);
    render(&context, &mut cache, presentation, &focus, size, vec![]);
    let (output, _) = render(&context, &mut cache, presentation, &focus, size, vec![]);
    let labels = crate::tests::painted_text(&output)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(labels.contains("CALLDATASIZE"));
    assert!(
        labels.contains("RETURN"),
        "decoded unexecuted source remains browseable"
    );
    assert!(
        !labels.contains("CALLDATALOAD"),
        "root code at the same PC must not leak into child source"
    );
    let pos = text_rect(&output, "0x0058  RETURN").center();
    for pressed in [true, false] {
        let (_, pick) = render(
            &context,
            &mut cache,
            presentation,
            &focus,
            size,
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
            let pick = pick.expect("actual decoded row click");
            assert_eq!(
                pick,
                SourcePick {
                    block: 9,
                    pc: Some(88)
                }
            );
            let selected = pick.focus(&focus);
            assert!(presentation.validate(&selected).is_ok());
            assert_eq!(selected.context, None);
            assert_eq!(selected.graph_selection(), None);
            assert_eq!(
                selected.source.as_ref().and_then(|source| source.program()),
                Some(7)
            );
        }
    }
}
