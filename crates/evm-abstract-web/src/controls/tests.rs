use egui::{
    Context, Event, FullOutput, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Response,
};

use super::{Density, FOCUS_STROKE, action, action_enabled, menu_button, tab_value, toggle};
use crate::palette;

struct State {
    enabled: bool,
    actions: usize,
    toggled: bool,
    tab: u8,
    menu_actions: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            enabled: true,
            actions: 0,
            toggled: false,
            tab: 0,
            menu_actions: 0,
        }
    }
}

struct Frame {
    output: FullOutput,
    controls: Vec<Response>,
    menu_item: Option<Response>,
    band: Rect,
}

fn frame(ctx: &Context, state: &mut State, density: Density, events: Vec<Event>) -> Frame {
    let mut controls = vec![];
    let mut menu_item = None;
    let mut band = Rect::NOTHING;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(700.0, 260.0))),
            events,
            ..RawInput::default()
        },
        |ui| {
            density.scope(ui, |ui| {
                ui.add_enabled_ui(state.enabled, |ui| {
                    band = ui
                        .horizontal(|ui| {
                            let response = action_enabled(ui, true, "Control", density);
                            if response.clicked() {
                                state.actions += 1;
                            }
                            controls.push(response);
                            controls.push(toggle(ui, &mut state.toggled, "Control", density));
                            controls.push(tab_value(ui, &mut state.tab, 0, "Control", density));
                            controls.push(tab_value(ui, &mut state.tab, 1, "Control", density));
                            controls.push(menu_button(ui, "Control", density, |ui| {
                                let response = action(ui, "Choose", density);
                                if response.clicked() {
                                    state.menu_actions += 1;
                                    ui.close();
                                }
                                menu_item = Some(response);
                            }));
                        })
                        .response
                        .rect;
                });
            });
        },
    );
    output.textures_delta.clear();
    Frame {
        output,
        controls,
        menu_item,
        band,
    }
}

fn settled(ctx: &Context, state: &mut State, density: Density) -> Frame {
    frame(ctx, state, density, vec![]);
    frame(ctx, state, density, vec![])
}

fn pointer(pos: Pos2, pressed: bool) -> Vec<Event> {
    vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        },
    ]
}

fn click(ctx: &Context, state: &mut State, density: Density, pos: Pos2) -> Frame {
    frame(ctx, state, density, pointer(pos, true));
    frame(ctx, state, density, pointer(pos, false))
}

fn key(key: Key) -> Vec<Event> {
    [true, false]
        .into_iter()
        .map(|pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        })
        .collect()
}

fn background(output: &FullOutput, response: &Response) -> egui::epaint::RectShape {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect) if rect.rect == response.rect => Some(rect.clone()),
            _ => None,
        })
        .expect("control must paint within its allocated rectangle")
}

fn focused(output: &FullOutput, response: &Response) -> bool {
    output.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::Shape::Rect(rect) if rect.rect == response.rect.shrink(2.0) && rect.stroke == FOCUS_STROKE
    ))
}

#[test]
fn actions_toggles_tabs_and_menu_triggers_share_geometry_in_both_densities() {
    for density in [Density::Regular, Density::Compact] {
        let ctx = Context::default();
        palette::configure(&ctx);
        let mut state = State::default();
        let initial = settled(&ctx, &mut state, density);
        let expected = initial.controls[0].rect.size();
        assert_eq!(expected.y, density.height());
        assert_eq!(
            initial.band.height(),
            density.height(),
            "compact rows must not reserve an invisible regular-height band"
        );
        for response in &initial.controls {
            assert_eq!(response.rect.size(), expected);
            let rect = background(&initial.output, response);
            assert_eq!(rect.corner_radius, egui::CornerRadius::same(4));
            let text = initial
                .output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text)
                        if response
                            .rect
                            .contains_rect(Rect::from_min_size(text.pos, text.galley.size())) =>
                    {
                        Some(text)
                    }
                    _ => None,
                })
                .unwrap();
            assert!(
                text.galley
                    .job
                    .sections
                    .iter()
                    .all(|section| section.format.font_id == density.font())
            );
            assert!(
                (text.pos.y + text.galley.size().y / 2.0 - response.rect.center().y).abs() < 0.01
            );
        }
        let pos = initial.controls[0].rect.center();
        let hover = frame(&ctx, &mut state, density, vec![Event::PointerMoved(pos)]);
        assert_eq!(
            background(&hover.output, &hover.controls[0]).fill,
            palette::CONTROL_HOVER
        );
        let down = frame(&ctx, &mut state, density, pointer(pos, true));
        assert_eq!(
            background(&down.output, &down.controls[0]).fill,
            palette::CONTROL_PRESSED
        );
        assert_eq!(down.controls[0].rect, initial.controls[0].rect);
        let release = frame(&ctx, &mut state, density, pointer(pos, false));
        assert!(release.controls[0].clicked());
        assert_eq!(state.actions, 1);
        for (actual, before) in release.controls.iter().zip(&initial.controls) {
            assert_eq!(
                actual.rect, before.rect,
                "interaction must not expand a control or move its neighbors"
            );
        }
    }
}

#[test]
fn toggle_inverts_but_selected_tabs_remain_selected() {
    for density in [Density::Regular, Density::Compact] {
        let ctx = Context::default();
        palette::configure(&ctx);
        let mut state = State::default();
        let initial = settled(&ctx, &mut state, density);
        let toggle_pos = initial.controls[1].rect.center();
        assert!(click(&ctx, &mut state, density, toggle_pos).controls[1].changed());
        assert!(state.toggled);
        frame(&ctx, &mut state, density, vec![Event::PointerGone]);
        let selected = settled(&ctx, &mut state, density);
        assert_eq!(
            background(&selected.output, &selected.controls[1]).fill,
            palette::SELECTED
        );
        assert!(click(&ctx, &mut state, density, toggle_pos).controls[1].changed());
        assert!(!state.toggled);
        let first = initial.controls[2].rect.center();
        assert!(!click(&ctx, &mut state, density, first).controls[2].changed());
        assert_eq!(state.tab, 0);
        let second = initial.controls[3].rect.center();
        assert!(click(&ctx, &mut state, density, second).controls[3].changed());
        assert_eq!(state.tab, 1);
        assert!(!click(&ctx, &mut state, density, second).controls[3].changed());
        assert_eq!(state.tab, 1);
        assert!(click(&ctx, &mut state, density, first).controls[2].changed());
        assert_eq!(state.tab, 0);
    }
}

#[test]
fn keyboard_traversal_paints_focus_separately_from_selection_and_activates_controls() {
    for density in [Density::Regular, Density::Compact] {
        let ctx = Context::default();
        palette::configure(&ctx);
        let mut state = State::default();
        settled(&ctx, &mut state, density);
        for index in 0..4 {
            frame(&ctx, &mut state, density, key(Key::Tab));
            let current = settled(&ctx, &mut state, density);
            assert!(
                current.controls[index].has_focus(),
                "Tab must reach control {index}"
            );
            assert!(focused(&current.output, &current.controls[index]));
            if index == 2 {
                assert_eq!(
                    background(&current.output, &current.controls[index]).fill,
                    palette::SELECTED
                );
            }
            if index == 0 {
                frame(&ctx, &mut state, density, key(Key::Enter));
                assert_eq!(state.actions, 1);
            } else if index == 1 {
                frame(&ctx, &mut state, density, key(Key::Space));
                assert!(state.toggled);
            } else if index == 3 {
                frame(&ctx, &mut state, density, key(Key::Enter));
                assert_eq!(state.tab, 1);
                let selected = settled(&ctx, &mut state, density);
                assert!(focused(&selected.output, &selected.controls[3]));
                assert_eq!(
                    background(&selected.output, &selected.controls[3]).fill,
                    palette::SELECTED
                );
                assert_eq!(
                    background(&selected.output, &selected.controls[2]).fill,
                    palette::RAISED
                );
            }
        }
    }
}

#[test]
fn disabled_controls_cannot_activate_or_paint_a_focus_ring() {
    let ctx = Context::default();
    palette::configure(&ctx);
    let density = Density::Compact;
    let mut state = State {
        enabled: false,
        ..State::default()
    };
    let initial = settled(&ctx, &mut state, density);
    for (index, response) in initial.controls.iter().enumerate() {
        ctx.memory_mut(|memory| memory.request_focus(response.id));
        frame(&ctx, &mut state, density, key(Key::Enter));
        let current = click(&ctx, &mut state, density, response.rect.center());
        assert!(!current.controls[index].enabled());
        assert!(!current.controls[index].clicked());
        assert!(!focused(&current.output, &current.controls[index]));
    }
    assert_eq!(state.actions, 0);
    assert!(!state.toggled);
    assert_eq!(state.tab, 0);
    assert_eq!(state.menu_actions, 0);
    assert!(settled(&ctx, &mut state, density).menu_item.is_none());
}

#[test]
fn shared_menu_trigger_opens_a_real_popup_and_action_closes_it() {
    let ctx = Context::default();
    palette::configure(&ctx);
    let density = Density::Regular;
    let mut state = State::default();
    let initial = settled(&ctx, &mut state, density);
    assert!(initial.menu_item.is_none());
    click(&ctx, &mut state, density, initial.controls[4].rect.center());
    frame(&ctx, &mut state, density, vec![Event::PointerGone]);
    let open = settled(&ctx, &mut state, density);
    assert_eq!(
        background(&open.output, &open.controls[4]).fill,
        palette::SELECTED
    );
    let item = open
        .menu_item
        .expect("menu button must open the framework popup");
    click(&ctx, &mut state, density, item.rect.center());
    assert_eq!(state.menu_actions, 1);
    assert!(settled(&ctx, &mut state, density).menu_item.is_none());
}

#[test]
fn long_menu_choices_honor_the_available_width_and_keep_their_label() {
    let ctx = Context::default();
    palette::configure(&ctx);
    let label = format!(
        "Archive RPC (https://example.org/{})",
        "long-path/".repeat(30)
    );
    let mut response = None;
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        ui.set_width(180.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        response = Some(super::tab(ui, true, &label, Density::Regular));
    });
    output.textures_delta.clear();
    let response = response.unwrap();
    assert!(response.rect.width() <= 180.0);
    assert_eq!(response.rect.height(), Density::Regular.height());
    let text = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        text.galley.text(),
        label,
        "elision must retain the original accessible label"
    );
    assert!(
        response
            .rect
            .contains_rect(Rect::from_min_size(text.pos, text.galley.size()))
    );
    assert!(
        text.galley
            .rows
            .iter()
            .flat_map(|row| &row.glyphs)
            .any(|glyph| glyph.chr == '…')
    );
}
