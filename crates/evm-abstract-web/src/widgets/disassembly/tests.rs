use super::natural_width;
use egui::{Context, RawInput, TextStyle};

#[test]
fn source_width_covers_one_complete_push_row_and_ignores_machine_selection() {
    let mut report = crate::tests::report();
    let instruction =
        &mut std::sync::Arc::make_mut(&mut report.model.programs[0]).blocks[0].instructions[0];
    instruction.name = "PUSH32".into();
    instruction.immediate = Some(format!("0x{}", "fe".repeat(32)));
    let line = super::copy::instruction(instruction);
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    let mut output = context.run_ui(RawInput::default(), |ui| {
        let line_width = ui
            .painter()
            .layout_no_wrap(
                line.clone(),
                TextStyle::Monospace.resolve(ui.style()),
                crate::palette::TEXT,
            )
            .size()
            .x;
        let width = natural_width(ui, &report.view(), Some(0), Default::default());
        let glyph_width = ui
            .painter()
            .layout_no_wrap(
                "0".into(),
                TextStyle::Monospace.resolve(ui.style()),
                crate::palette::TEXT,
            )
            .size()
            .x;
        // The production width estimates mono columns; galley bounds include
        // pixel-rounded bearings. An extra PC column exceeds this allowance.
        assert!(
            (width - line_width - 76.0).abs() < glyph_width,
            "one full PC/opcode/operand line plus controls: {width} vs {line_width}"
        );
        assert_eq!(
            width,
            natural_width(
                ui,
                &report.view(),
                Some(0),
                crate::app::Selection {
                    state: Some(usize::MAX),
                    ..Default::default()
                }
            )
        );
        assert_eq!(
            natural_width(ui, &report.view(), Some(999), Default::default()),
            200.0
        );
    });
    output.textures_delta.clear();
}
