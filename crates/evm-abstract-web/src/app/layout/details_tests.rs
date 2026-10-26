use egui::{Context, Rect, Vec2};
use evm_abstract_protocol::AnalyzeReply;

use super::tests::{click, settled, texts};
use crate::{app::View, ssa_display::SsaDisplay, tests::ready};

#[test]
fn details_button_is_reachable_and_shared_across_views_widths_and_reports() {
    for width in [320.0, 640.0, 1024.0, 1440.0] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        let mut workspace = ready();
        workspace.view = View::Ssa;
        let size = Vec2::new(width, 800.0);
        let output = settled(&ctx, &mut workspace, size);
        assert_eq!(workspace.ssa_display, SsaDisplay::Compact);
        assert!(
            workspace
                .accessible_status()
                .contains("SSA details: Compact")
        );
        let button = texts(&output)
            .into_iter()
            .find(|(text, _)| text == "SSA details")
            .expect("the result bar must offer the details toggle")
            .1;
        assert!(Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(button));
        assert!(
            !texts(&output)
                .iter()
                .any(|(text, _)| text.contains("Completed"))
        );

        click(&ctx, &mut workspace, size, button.center());
        let output = settled(&ctx, &mut workspace, size);
        assert_eq!(workspace.ssa_display, SsaDisplay::Full);
        assert!(workspace.accessible_status().contains("SSA details: Full"));
        assert!(
            texts(&output)
                .iter()
                .any(|(text, _)| text.contains("Completed"))
        );

        for view in [View::Split, View::Disassembly, View::Graph, View::Ssa] {
            workspace.view = view;
            for resize in [1440.0, 640.0, width] {
                settled(&ctx, &mut workspace, Vec2::new(resize, size.y));
                assert_eq!(workspace.ssa_display, SsaDisplay::Full);
            }
        }
        workspace.initial_command();
        workspace.receive(Ok(AnalyzeReply {
            result: Ok((crate::tests::report()).into()),
        }));
        let output = settled(&ctx, &mut workspace, size);
        assert_eq!(workspace.ssa_display, SsaDisplay::Full);
        assert!(
            texts(&output)
                .iter()
                .any(|(text, _)| text.contains("Completed"))
        );
        let button = texts(&output)
            .into_iter()
            .find(|(text, _)| text == "SSA details")
            .unwrap()
            .1;
        click(&ctx, &mut workspace, size, button.center());
        let output = settled(&ctx, &mut workspace, size);
        assert_eq!(workspace.ssa_display, SsaDisplay::Compact);
        assert!(
            !texts(&output)
                .iter()
                .any(|(text, _)| text.contains("Completed"))
        );
    }
}
