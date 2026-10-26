use super::*;
use egui::{Context, Modifiers, Pos2, Rect, Vec2};

fn press(pos: Pos2) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }
}

fn draw(ctx: &Context, events: Vec<Event>, time: f64) -> Vec2 {
    let mut delta = Vec2::ZERO;
    let mut output = ctx.run_ui(
        RawInput {
            events,
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(1000.0))),
            ..Default::default()
        },
        |ui| {
            delta = ui.input(|input| input.pointer.delta());
        },
    );
    output.textures_delta.clear();
    delta
}

#[test]
fn contact_position_does_not_become_a_drag_from_stale_mouse_hover() {
    let ctx = Context::default();
    draw(&ctx, vec![Event::PointerMoved(egui::pos2(2.0, 2.0))], 0.0);
    let mut contact = ContactStart::default();
    let pos = egui::pos2(84.0, 916.0);
    let mut input = RawInput {
        events: vec![press(pos)],
        ..Default::default()
    };
    assert!(contact.prepare(&mut input, true));
    draw(&ctx, input.events, 0.1);
    assert!(!ctx.input(|input| input.pointer.primary_down()));
    let mut input = RawInput::default();
    assert!(!contact.prepare(&mut input, false));
    assert_eq!(draw(&ctx, input.events, 0.2), Vec2::ZERO);
    assert!(ctx.input(|input| input.pointer.primary_pressed()));
    assert_eq!(
        draw(
            &ctx,
            vec![Event::PointerMoved(pos - egui::vec2(0.0, 40.0))],
            0.3
        ),
        egui::vec2(0.0, -40.0)
    );
}

#[test]
fn mouse_press_and_ongoing_contact_events_are_unchanged() {
    let mut contact = ContactStart::default();
    let events = vec![
        press(egui::pos2(20.0, 30.0)),
        Event::PointerMoved(egui::pos2(40.0, 50.0)),
    ];
    let mut input = RawInput {
        events: events.clone(),
        ..Default::default()
    };
    assert!(!contact.prepare(&mut input, false));
    assert_eq!(input.events, events);
}

fn scroll(ctx: &Context, events: Vec<Event>, time: f64) -> (f32, egui::Id) {
    let mut result = (0.0, egui::Id::NULL);
    let mut output = ctx.run_ui(
        RawInput {
            events,
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(1000.0))),
            ..Default::default()
        },
        |ui| {
            let area = egui::ScrollArea::vertical()
                .id_salt("contact-scroll")
                .max_height(500.0)
                .show(ui, |ui| {
                    ui.allocate_space(egui::vec2(200.0, 3000.0));
                });
            result = (area.state.offset.y, area.id);
        },
    );
    output.textures_delta.clear();
    result
}

#[test]
fn new_contact_keeps_scrolled_content_then_real_movement_scrolls_it() {
    for normalize in [false, true] {
        let ctx = Context::default();
        let (_, id) = scroll(&ctx, vec![], 0.0);
        scroll(&ctx, vec![], 0.1);
        let mut state = egui::scroll_area::State::load(&ctx, id).unwrap();
        state.offset.y = 300.0;
        state.store(&ctx, id);
        let (before, _) = scroll(&ctx, vec![Event::PointerMoved(egui::pos2(2.0, 2.0))], 0.2);
        assert_eq!(before, 300.0);

        let pos = egui::pos2(84.0, 450.0);
        let mut contact = ContactStart::default();
        let mut input = RawInput {
            events: vec![press(pos)],
            ..Default::default()
        };
        if normalize {
            assert!(contact.prepare(&mut input, true));
        }
        let (after_contact, _) = scroll(&ctx, input.events, 0.3);
        if !normalize {
            // Reproduce eframe's immediate pointerdown: the distant hover is
            // counted as a drag and resets the already-scrolled content.
            assert_eq!(after_contact, 0.0);
            continue;
        }
        assert_eq!(after_contact, before);
        // eframe can also enqueue the same press from touchstart. Neither
        // delivery may create motion before the actual finger moves.
        let mut input = RawInput {
            events: vec![press(pos)],
            ..Default::default()
        };
        assert!(!contact.prepare(&mut input, false));
        assert_eq!(scroll(&ctx, input.events, 0.4).0, before);
        assert_eq!(
            scroll(
                &ctx,
                vec![Event::PointerMoved(pos - egui::vec2(0.0, 40.0))],
                0.5,
            )
            .0,
            before + 40.0,
        );
    }
}

#[test]
fn release_before_next_paint_retains_the_real_click() {
    let ctx = Context::default();
    draw(&ctx, vec![Event::PointerMoved(egui::pos2(2.0, 2.0))], 0.0);
    let mut contact = ContactStart::default();
    let pos = egui::pos2(84.0, 450.0);
    let mut input = RawInput {
        events: vec![press(pos)],
        ..Default::default()
    };
    assert!(contact.prepare(&mut input, true));
    draw(&ctx, input.events, 0.1);
    let mut input = RawInput {
        events: vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        ..Default::default()
    };
    assert!(!contact.prepare(&mut input, false));
    draw(&ctx, input.events, 0.2);
    assert!(ctx.input(|input| input.pointer.primary_clicked()));
    assert!(!ctx.input(|input| input.pointer.primary_down()));
}
