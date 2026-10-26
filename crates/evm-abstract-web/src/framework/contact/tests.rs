use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn draw(ctx: &Context, events: Vec<Event>, time: f64) -> (f32, egui::Id) {
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
                // Exercise the same drag path as a browser that has already
                // seen a touch device, without changing global input state.
                .scroll_source(egui::scroll_area::ScrollSource::ALL)
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
    let ctx = Context::default();
    let (_, id) = draw(&ctx, vec![], 0.0);
    draw(&ctx, vec![], 0.1);
    let mut state = egui::scroll_area::State::load(&ctx, id).unwrap();
    state.offset.y = 300.0;
    state.store(&ctx, id);
    let (before, _) = draw(&ctx, vec![Event::PointerMoved(egui::pos2(2.0, 2.0))], 0.2);
    assert_eq!(before, 300.0);

    // eframe delivers pointerdown synchronously, before touchstart metadata.
    let pos = egui::pos2(84.0, 450.0);
    assert_eq!(draw(&ctx, vec![button(pos, true)], 0.3).0, before);
    assert!(ctx.input(|input| input.pointer.primary_pressed()));
    assert_eq!(ctx.input(|input| input.pointer.delta()), Vec2::ZERO);
    // Its later touchstart may repeat the press. The real movement then scrolls.
    assert_eq!(draw(&ctx, vec![button(pos, true)], 0.4).0, before);
    assert_eq!(
        draw(
            &ctx,
            vec![Event::PointerMoved(pos - egui::vec2(0.0, 40.0))],
            0.5,
        )
        .0,
        before + 40.0,
    );
}

#[test]
fn press_keeps_later_movement_and_duplicate_press_does_not_reanchor() {
    let ctx = Context::default();
    draw(&ctx, vec![Event::PointerMoved(egui::pos2(2.0, 2.0))], 0.0);
    let pos = egui::pos2(84.0, 450.0);
    draw(
        &ctx,
        vec![
            button(pos, true),
            Event::PointerMoved(pos - egui::vec2(0.0, 40.0)),
        ],
        0.1,
    );
    assert_eq!(
        ctx.input(|input| input.pointer.delta()),
        egui::vec2(0.0, -40.0),
    );
    let next = pos - egui::vec2(0.0, 80.0);
    draw(
        &ctx,
        vec![Event::PointerMoved(next), button(next, true)],
        0.2,
    );
    assert_eq!(
        ctx.input(|input| input.pointer.delta()),
        egui::vec2(0.0, -40.0),
    );
    assert!(ctx.input(|input| input.pointer.primary_down()));
}

#[test]
fn synchronous_click_and_double_click_history_are_retained() {
    let ctx = Context::default();
    draw(&ctx, vec![Event::PointerMoved(egui::pos2(2.0, 2.0))], 0.0);
    let pos = egui::pos2(84.0, 450.0);
    draw(&ctx, vec![button(pos, true), button(pos, false)], 0.1);
    assert!(ctx.input(|input| input.pointer.primary_clicked()));
    assert!(!ctx.input(|input| input.pointer.primary_down()));
    draw(&ctx, vec![button(pos, true), button(pos, false)], 0.2);
    assert!(ctx.input(|input| input.pointer.button_double_clicked(PointerButton::Primary)));
}

#[test]
fn hover_motion_remains_motion_without_a_new_press() {
    let ctx = Context::default();
    draw(&ctx, vec![Event::PointerMoved(egui::pos2(20.0, 30.0))], 0.0);
    draw(&ctx, vec![Event::PointerMoved(egui::pos2(40.0, 50.0))], 0.1);
    assert_eq!(
        ctx.input(|input| input.pointer.delta()),
        egui::vec2(20.0, 20.0),
    );
}
