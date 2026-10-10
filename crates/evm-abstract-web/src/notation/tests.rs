use super::{INDEX_SCALE, job, widget};
use crate::palette;
use egui::{Align, Context, FontFamily, FontId, RawInput, RichText};
use evm_abstract_notation::Symbol;

#[test]
fn index_runs_use_smaller_ordinary_digits_and_preserve_ssa_value_names() {
    let text = format!(
        "{} → {} · {} = φ(%128)",
        Symbol::State(0),
        Symbol::State(128),
        Symbol::Effect(17)
    );
    let job = job(&text, FontId::monospace(12.0), palette::TEXT);
    assert_eq!(job.text, text);
    let indices = job
        .sections
        .iter()
        .filter(|section| section.format.valign == Align::BOTTOM)
        .collect::<Vec<_>>();
    assert_eq!(indices.len(), 3);
    for section in indices {
        assert_eq!(
            section.format.font_id,
            FontId::new(
                12.0 * INDEX_SCALE,
                FontFamily::Name(super::MATH_MONO.into())
            )
        );
        assert!(
            job.text[section.byte_range.start.0..section.byte_range.end.0]
                .chars()
                .all(|digit| evm_abstract_notation::subscript_digit(digit).is_some())
        );
    }
    let values = job
        .sections
        .iter()
        .find(|section| {
            job.text[section.byte_range.start.0..section.byte_range.end.0].contains("%128")
        })
        .unwrap();
    assert_eq!(values.format.font_id.size, 12.0);
    assert_ne!(values.format.valign, Align::BOTTOM);
}

#[test]
fn real_font_layout_lowers_indices_and_keeps_all_digits_out_of_missing_glyphs() {
    let ctx = Context::default();
    palette::configure(&ctx);
    let mut output = ctx.run_ui(RawInput::default(), |_| {});
    output.textures_delta.clear();
    ctx.fonts_mut(|fonts| {
        for family in [FontFamily::Monospace, FontFamily::Proportional] {
            let galley = fonts.layout_job(job(
                "σ₀₁₂₃₄₅₆₇₈₉",
                FontId::new(16.0, family.clone()),
                palette::TEXT,
            ));
            let glyphs = &galley.rows[0].glyphs;
            assert_eq!(glyphs.len(), 11);
            assert_eq!(glyphs[0].chr, 'σ');
            let missing =
                fonts.layout_job(job("\u{10ffff}", FontId::new(12.0, family), palette::TEXT));
            for glyph in &glyphs[1..] {
                assert!(evm_abstract_notation::subscript_digit(glyph.chr).is_some());
                assert!(glyph.font_height < glyphs[0].font_height);
                assert!(glyph.logical_rect().top() > glyphs[0].logical_rect().top());
                assert_ne!(glyph.uv_rect, missing.rows[0].glyphs[0].uv_rect);
            }
            assert!(galley.num_indices > 0 && galley.size().x > 0.0);
        }
    });
}

#[test]
fn widget_typesetting_preserves_styles_colors_and_plain_text() {
    let ctx = Context::default();
    palette::configure(&ctx);
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        let styled = widget(
            ui,
            RichText::new(Symbol::State(128).to_string())
                .monospace()
                .size(18.0)
                .color(palette::WARNING),
        );
        let egui::WidgetText::LayoutJob(job) = styled else {
            panic!("indexed widget must carry a job");
        };
        assert_eq!(job.text, "σ₁₂₈");
        for section in &job.sections {
            assert_eq!(section.format.color, palette::WARNING);
        }
        assert_eq!(job.sections[0].format.font_id.family, FontFamily::Monospace);
        assert_eq!(
            job.sections[1].format.font_id.family,
            FontFamily::Name(super::MATH_MONO.into())
        );
        assert_eq!(job.sections[0].format.font_id.size, 18.0);
        assert_eq!(job.sections[1].format.font_id.size, 18.0 * INDEX_SCALE);
        let plain = widget(ui, "%128 = ADD 0x40 2048");
        assert_eq!(plain.text(), "%128 = ADD 0x40 2048");
        assert!(matches!(plain, egui::WidgetText::Text(_)));
    });
    output.textures_delta.clear();
}

#[test]
fn projected_state_typesets_its_namespace_above_the_native_index() {
    let ctx = Context::default();
    palette::configure(&ctx);
    let mut output = ctx.run_ui(RawInput::default(), |_| {});
    output.textures_delta.clear();
    ctx.fonts_mut(|fonts| {
        for family in [FontFamily::Monospace, FontFamily::Proportional] {
            let text = Symbol::ProjectedState(128).to_string();
            let layout = job(&text, FontId::new(16.0, family.clone()), palette::TEXT);
            assert_eq!(layout.text, "σᵖ₁₂₈");
            assert_eq!(layout.sections[1].format.valign, Align::TOP);
            assert_eq!(layout.sections[2].format.valign, Align::BOTTOM);
            let galley = fonts.layout_job(layout);
            let glyphs = &galley.rows[0].glyphs;
            assert_eq!(glyphs[1].chr, 'ᵖ');
            assert!(glyphs[1].font_height < glyphs[0].font_height);
            assert!(glyphs[1].logical_rect().top() < glyphs[2].logical_rect().top());
            let missing =
                fonts.layout_job(job("\u{10ffff}", FontId::new(12.0, family), palette::TEXT));
            assert_ne!(glyphs[1].uv_rect, missing.rows[0].glyphs[0].uv_rect);
        }
    });
}
