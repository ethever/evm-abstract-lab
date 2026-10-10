use egui::{Context, FontDefinitions, FontFamily, FontId, Pos2, RawInput, Vec2};

use super::{TEXT, configure};

fn initialize(ctx: &Context) {
    let mut output = ctx.run_ui(RawInput::default(), |_| {});
    output.textures_delta.clear();
}

#[test]
fn default_fonts_reproduce_the_missing_proportional_status_and_stack_symbols() {
    let ctx = Context::default();
    initialize(&ctx);
    ctx.fonts_mut(|fonts| {
        for symbol in ['→', '●'] {
            assert!(
                !fonts
                    .fonts
                    .font(&FontFamily::Proportional)
                    .characters()
                    .contains_key(&symbol),
                "the pinned default proportional family unexpectedly gained {symbol}"
            );
            assert!(
                fonts
                    .fonts
                    .font(&FontFamily::Monospace)
                    .characters()
                    .contains_key(&symbol),
                "missing baseline monospace {symbol}"
            );
        }
    });
}

#[test]
fn configured_families_resolve_and_shape_workbench_symbols_without_replacement_glyphs() {
    let ctx = Context::default();
    configure(&ctx);
    initialize(&ctx);
    ctx.fonts_mut(|fonts| {
        for font in [FontId::proportional(14.0), FontId::monospace(14.0)] {
            for symbol in ['→', '●', 'μ', 'φ'] {
                // epaint 0.36 has_glyph compares face IDs, giving false
                // negatives when a valid glyph shares the replacement face.
                // Inspect the loaded family charmap and real shaped geometry.
                assert!(
                    fonts
                        .fonts
                        .font(&font.family)
                        .characters()
                        .contains_key(&symbol),
                    "missing {symbol} in {font:?}"
                );
                let galley = fonts.layout_no_wrap(symbol.to_string(), font.clone(), TEXT);
                assert!(galley.num_indices > 0 && galley.num_vertices > 0);
                assert!(galley.size().x > 0.0 && galley.size().y > 0.0);
                assert_eq!(galley.rows[0].glyphs[0].chr, symbol);
                let missing = fonts.layout_no_wrap("\u{10ffff}".into(), font.clone(), TEXT);
                assert_ne!(
                    galley.rows[0].glyphs[0].uv_rect, missing.rows[0].glyphs[0].uv_rect,
                    "{symbol} must rasterize its own glyph, not the missing-character box"
                );
            }
        }
    });
}

fn shaped_metrics(ctx: &Context, font: FontId, text: &str) -> (Vec2, Vec<(char, Pos2, f32, f32)>) {
    ctx.fonts_mut(|fonts| {
        let galley = fonts.layout_no_wrap(text.into(), font, TEXT);
        (
            galley.size(),
            galley
                .rows
                .iter()
                .flat_map(|row| &row.glyphs)
                .map(|glyph| (glyph.chr, glyph.pos, glyph.advance_width, glyph.font_height))
                .collect(),
        )
    })
}

#[test]
fn fallback_preserves_existing_font_order_data_and_text_metrics() {
    let default = Context::default();
    initialize(&default);
    let configured = Context::default();
    configure(&configured);
    initialize(&configured);
    let original = FontDefinitions::default();
    configured.fonts(|fonts| {
        let definitions = fonts.definitions();
        for (name, data) in &original.font_data {
            assert_eq!(&definitions.font_data[name], data);
        }
        assert_eq!(definitions.font_data.len(), original.font_data.len() + 2);
        assert_eq!(
            definitions.families[&FontFamily::Monospace],
            original.families[&FontFamily::Monospace]
        );
        let proportional = &definitions.families[&FontFamily::Proportional];
        assert!(proportional.starts_with(&original.families[&FontFamily::Proportional]));
        assert_eq!(
            proportional
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            proportional.len()
        );
    });
    for (font, text) in [
        (
            FontId::monospace(12.0),
            "PUSH1 0x20  %1 = ADD %2 %3  μ0 φ 2 → 1",
        ),
        (
            FontId::proportional(14.0),
            "Model converged / Analyze / Disassembly",
        ),
    ] {
        assert_eq!(
            shaped_metrics(&configured, font.clone(), text),
            shaped_metrics(&default, font, text)
        );
    }
}
