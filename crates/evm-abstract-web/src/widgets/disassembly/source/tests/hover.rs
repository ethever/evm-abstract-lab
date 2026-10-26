use super::*;

#[test]
fn decoded_stack_signatures_and_explanations_are_hover_only_without_native_borrowing() {
    for (opcode, name, inputs, outputs, explanation) in [
        (0x60, "PUSH1", 0, 1, "Push the immediate value"),
        (0x01, "ADD", 2, 1, "Pop two values and push their sum"),
        (0x50, "POP", 1, 0, "Pop and discard the top value"),
        (0x80, "DUP1", 1, 2, "Copy stack position 1 (1 = top)"),
        (
            0x90,
            "SWAP1",
            2,
            2,
            "Exchange the top with stack position 2",
        ),
        (0x8f, "DUP16", 16, 17, "Copy stack position 16 (1 = top)"),
        (
            0x9f,
            "SWAP16",
            17,
            17,
            "Exchange the top with stack position 17",
        ),
    ] {
        let mut report = crate::tests::report();
        let instruction =
            &mut std::sync::Arc::make_mut(&mut report.model.programs[0]).blocks[0].instructions[0];
        instruction.opcode = opcode;
        instruction.name = name.into();
        instruction.stack_inputs = inputs;
        instruction.stack_outputs = outputs;
        instruction.valid = true;
        let label = super::super::super::copy::instruction(instruction);
        let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
        let mut index = ReportIndex::default();
        index.update(&report.model, view, None, None);
        let presentation = Presentation::new(&report.model, &index, view, None);
        // No context is selected. Another state's current SSA receipt is not
        // evidence for this decoded source row, including the ADD case.
        let focus = Focus::source(view, presentation.source(0).unwrap());
        let context = Context::default();
        crate::palette::configure(&context);
        context.style_mut_of(egui::Theme::Dark, |style| {
            style.interaction.tooltip_delay = 0.0
        });
        let mut cache = SourceCache::default();
        let mut frame = |time, events| {
            let mut output = context.run_ui(
                RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700.0, 450.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    cache.show(ui, presentation, &focus);
                },
            );
            output.textures_delta.clear();
            output
        };
        frame(0.0, vec![]);
        let idle = frame(0.1, vec![]);
        let idle_text = crate::tests::painted_text(&idle)
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
            .join("\n");
        let signature = format!("{inputs} → {outputs}");
        assert!(!idle_text.contains("Stack signature") && !idle_text.contains(&signature));
        let pos = text_rect(&idle, &label).center();
        frame(0.2, vec![egui::Event::PointerMoved(pos)]);
        frame(1.0, vec![]);
        let hover = frame(1.1, vec![]);
        let hover_text = crate::tests::painted_text(&hover)
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            signature.as_str(),
            explanation,
            "not the total stack heights",
            "Decoded source; no current execution receipt",
        ] {
            assert!(
                hover_text.contains(expected),
                "{name} missing {expected}: {hover_text}"
            );
        }
        assert!(!hover_text.contains("Observed in current execution evidence"));
    }
}
