use crate::decompile::form::Form;
use egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

#[test]
fn valid_prefix_and_whitespace_drafts_send_parser_compatible_payloads_without_rewriting_edits() {
    for (draft, expected) in [
        ("0x600160020100", "0x600160020100"),
        (" \n0X60 01\t60 02 01 00 ", "0x600160020100"),
        ("60 01\n60 02 01 00", "600160020100"),
    ] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        let mut form = Form::default();
        form.load(draft.into(), evm_abstract_protocol::Fork::Osaka);
        assert!(form.validation().is_none());
        let mut initial = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 800.0))),
                ..RawInput::default()
            },
            |ui| {
                assert!(form.show(ui, false).is_none());
            },
        );
        initial.textures_delta.clear();
        let mut request = None;
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 800.0))),
                events: vec![Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::CTRL,
                }],
                ..RawInput::default()
            },
            |ui| request = form.show(ui, false),
        );
        output.textures_delta.clear();
        assert_eq!(
            request
                .expect("valid draft submits by the real shortcut")
                .bytecode,
            expected
        );
        assert_eq!(form.bytecode, draft);
    }
}
