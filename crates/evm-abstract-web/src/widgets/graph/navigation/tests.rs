use egui::{Context, Event, Modifiers, MouseWheelUnit, Pos2, RawInput, Rect, TouchPhase, Vec2};

use super::super::Graph;
use crate::{app::Selection, tests::report};

fn frame(ctx: &Context, graph: &mut Graph, events: Vec<Event>, sidebar: bool) -> (Rect, f32) {
    let mut canvas = Rect::NOTHING;
    let mut source_offset = 0.0;
    let mut raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))),
        events,
        ..RawInput::default()
    };
    graph.prepare_input(&mut raw);
    let mut output = ctx.run_ui(raw, |ui| {
        if sidebar {
            egui::Panel::left("source")
                .exact_size(240.0)
                .show(ui, |ui| {
                    let scroll = egui::ScrollArea::vertical().show(ui, |ui| {
                        for index in 0..100 {
                            ui.label(format!("Source row {index}"));
                        }
                    });
                    source_offset = scroll.state.offset.y;
                });
        }
        let available = ui.available_rect_before_wrap();
        graph.show(ui, &(report()).view(), &mut Selection::default());
        let size = graph.viewport.unwrap();
        canvas = Rect::from_min_size(available.right_bottom() - size, size);
    });
    output.textures_delta.clear();
    (canvas, source_offset)
}

fn wheel(delta: Vec2, modifiers: Modifiers) -> Event {
    Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta,
        phase: TouchPhase::Move,
        modifiers,
    }
}

#[test]
fn trackpad_scroll_pans_both_axes_without_zoom_even_over_a_node() {
    for over_node in [false, true] {
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        let mut graph = Graph::default();
        graph.set_mode(crate::widgets::GraphMode::States);
        frame(&ctx, &mut graph, vec![], false);
        let (canvas, _) = frame(&ctx, &mut graph, vec![], false);
        let pointer = if over_node {
            graph
                .screen_rect(canvas, graph.scene.placement.nodes[&0])
                .center()
        } else {
            canvas.right_bottom() - Vec2::splat(12.0)
        };
        frame(&ctx, &mut graph, vec![Event::PointerMoved(pointer)], false);
        let pan = graph.pan;
        let zoom = graph.zoom;
        let delta = Vec2::new(4.0, -3.0);
        frame(&ctx, &mut graph, vec![wheel(delta, Modifiers::NONE)], false);
        assert_eq!(graph.zoom, zoom, "ordinary trackpad scroll must not zoom");
        assert!(
            (graph.pan - pan - delta).length() < 0.001,
            "pan missing over_node={over_node}"
        );
        assert!(!graph.fitted);
    }
}

#[test]
fn pinch_and_modified_scroll_zoom_around_pointer_without_scroll_translation() {
    for event in [
        Event::Zoom(1.25),
        wheel(
            Vec2::new(0.0, 5.0),
            Modifiers {
                ctrl: true,
                command: true,
                ..Modifiers::NONE
            },
        ),
        wheel(
            Vec2::new(0.0, 5.0),
            Modifiers {
                mac_cmd: true,
                command: true,
                ..Modifiers::NONE
            },
        ),
    ] {
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        let mut graph = Graph::default();
        graph.set_mode(crate::widgets::GraphMode::States);
        frame(&ctx, &mut graph, vec![], false);
        let (canvas, _) = frame(&ctx, &mut graph, vec![], false);
        let pointer = canvas.min + Vec2::new(180.0, 170.0);
        frame(&ctx, &mut graph, vec![Event::PointerMoved(pointer)], false);
        let world = (pointer - canvas.min - graph.pan) / graph.zoom;
        let zoom = graph.zoom;
        frame(&ctx, &mut graph, vec![event], false);
        assert!(graph.zoom > zoom);
        assert!(((pointer - canvas.min - graph.pan) / graph.zoom - world).length() < 0.001);
        assert!(!graph.fitted);
    }
}

#[test]
fn symmetric_touch_pinch_keeps_the_gesture_center_anchored() {
    use egui::{TouchDeviceId, TouchId};

    for (factor, translation, over_node, pointer_drag) in [
        (1.12, Vec2::ZERO, false, false),
        (0.84, Vec2::ZERO, false, false),
        (1.12, Vec2::ZERO, false, true),
        (0.84, Vec2::ZERO, false, true),
        (1.12, Vec2::new(18.0, 11.0), false, true),
        (1.12, Vec2::ZERO, true, true),
        (0.84, Vec2::ZERO, true, true),
        (1.12, Vec2::new(18.0, 11.0), true, true),
    ] {
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        let mut graph = Graph::default();
        graph.set_mode(crate::widgets::GraphMode::States);
        frame(&ctx, &mut graph, vec![], false);
        let (canvas, _) = frame(&ctx, &mut graph, vec![], false);
        let center = if over_node {
            graph
                .screen_rect(canvas, graph.scene.placement.nodes[&0])
                .center()
        } else {
            canvas.right_bottom() - Vec2::new(120.0, 90.0)
        };
        let radius = Vec2::new(60.0, 0.0);
        frame(
            &ctx,
            &mut graph,
            vec![Event::PointerMoved(center - radius)],
            false,
        );
        let touch = |id, phase, pos| Event::Touch {
            device_id: TouchDeviceId(0),
            id: TouchId(id),
            phase,
            pos,
            force: None,
        };
        let mut start = vec![
            Event::PointerMoved(center - radius),
            touch(0, TouchPhase::Start, center - radius),
            touch(1, TouchPhase::Start, center + radius),
        ];
        if pointer_drag {
            start.insert(
                1,
                Event::PointerButton {
                    pos: center - radius,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            );
        }
        frame(&ctx, &mut graph, start, false);
        assert!(ctx.input(|input| input.multi_touch().is_some()));
        let world = (center - canvas.min - graph.pan) / graph.zoom;
        let zoom = graph.zoom;
        let moved = center + translation;
        frame(
            &ctx,
            &mut graph,
            vec![
                Event::PointerMoved(moved - radius * factor),
                touch(0, TouchPhase::Move, moved - radius * factor),
                touch(1, TouchPhase::Move, moved + radius * factor),
            ],
            false,
        );
        assert!(
            (graph.zoom / zoom - factor).abs() < 0.0001,
            "expected touch scale {factor}, actual {}",
            graph.zoom / zoom
        );
        assert!(
            ((moved - canvas.min - graph.pan) / graph.zoom - world).length() < 0.001,
            "touch center must follow the gesture; over_node={over_node}, pointer_drag={pointer_drag}"
        );
        assert!(!graph.fitted);
    }
}

#[test]
fn scrolling_a_neighbor_pane_leaves_the_graph_camera_untouched() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = Graph::default();
    graph.set_mode(crate::widgets::GraphMode::States);
    frame(&ctx, &mut graph, vec![], true);
    frame(&ctx, &mut graph, vec![], true);
    let pan = graph.pan;
    let zoom = graph.zoom;
    frame(
        &ctx,
        &mut graph,
        vec![Event::PointerMoved(Pos2::new(90.0, 300.0))],
        true,
    );
    let (_, offset) = frame(
        &ctx,
        &mut graph,
        vec![wheel(Vec2::new(0.0, -5.0), Modifiers::NONE)],
        true,
    );
    assert!(offset > 0.0, "source pane must still receive its scroll");
    assert_eq!(graph.pan, pan);
    assert_eq!(graph.zoom, zoom);
    assert!(!graph.fitted);
}

#[test]
fn fitting_over_a_neighbor_preserves_its_wheel_tail_and_fresh_input() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut graph = Graph::default();
    graph.set_mode(crate::widgets::GraphMode::States);
    frame(&ctx, &mut graph, vec![], true);
    frame(&ctx, &mut graph, vec![], true);
    graph.zoom_at(Vec2::new(180.0, 150.0), 0.5);
    let (_, first) = frame(
        &ctx,
        &mut graph,
        vec![
            Event::PointerMoved(Pos2::new(90.0, 300.0)),
            wheel(Vec2::new(0.0, -120.0), Modifiers::NONE),
        ],
        true,
    );
    assert!(first > 0.0 && first < 120.0);
    let key = |pressed| Event::Key {
        key: egui::Key::F,
        physical_key: Some(egui::Key::F),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let (_, after_fit) = frame(&ctx, &mut graph, vec![key(true)], true);
    assert!(graph.fitted);
    assert!(after_fit > first);
    let (_, fresh) = frame(
        &ctx,
        &mut graph,
        vec![key(false), wheel(Vec2::new(0.0, -5.0), Modifiers::NONE)],
        true,
    );
    assert!(
        fresh > after_fit,
        "the neighbor must retain its fresh wheel event"
    );
    for _ in 0..40 {
        frame(&ctx, &mut graph, vec![], true);
    }
    let (_, settled) = frame(&ctx, &mut graph, vec![], true);
    assert!(
        (settled - 125.0).abs() < 0.01,
        "Fit cancelled the neighbor's prior wheel tail: final source offset {settled}"
    );
    assert!(graph.fitted);
}

#[test]
fn fit_shortcut_takes_priority_over_wheel_and_zoom_smoothing() {
    for same_frame in [false, true] {
        for modifiers in [
            Modifiers::NONE,
            Modifiers {
                ctrl: true,
                command: true,
                ..Modifiers::NONE
            },
        ] {
            for resume_immediately in [false, true] {
                let ctx = Context::default();
                crate::notation::initialize_fonts(&ctx);
                let mut graph = Graph::default();
                graph.set_mode(crate::widgets::GraphMode::States);
                frame(&ctx, &mut graph, vec![], false);
                let (canvas, _) = frame(&ctx, &mut graph, vec![], false);
                let fitted_pan = graph.pan;
                let fitted_zoom = graph.zoom;
                let key = |pressed| Event::Key {
                    key: egui::Key::F,
                    physical_key: Some(egui::Key::F),
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                };
                let mut gesture = vec![
                    Event::PointerMoved(canvas.center()),
                    wheel(Vec2::new(-120.0, -60.0), modifiers),
                ];
                if same_frame {
                    // The explicit fit command wins over input already queued in
                    // this frame, whether it would pan or zoom the canvas.
                    gesture.push(key(true));
                    frame(&ctx, &mut graph, gesture, false);
                } else {
                    frame(&ctx, &mut graph, gesture, false);
                    assert!(!graph.fitted);
                    assert!(graph.pan != fitted_pan || graph.zoom != fitted_zoom);
                    // Keep the pointer in the canvas and press F before the prior
                    // wheel animation settles. No new gesture follows the command.
                    frame(&ctx, &mut graph, vec![key(true)], false);
                }
                assert!(
                    graph.fitted,
                    "queued wheel input cancelled F: {modifiers:?}, same_frame={same_frame}"
                );
                let mut fresh_input = vec![key(false)];
                if !resume_immediately {
                    frame(&ctx, &mut graph, fresh_input, false);
                    fresh_input = Vec::new();
                    for _ in 0..40 {
                        frame(&ctx, &mut graph, vec![], false);
                        assert!(graph.fitted, "old smoothing resumed without a new gesture");
                        assert!((graph.pan - fitted_pan).length() < 0.01);
                        assert_eq!(graph.zoom, fitted_zoom);
                    }
                }
                // On immediate resumption the hook must cancel the old wheel
                // animation before handling the key-up and new opposing wheel.
                let delta = Vec2::new(4.0, -3.0);
                fresh_input.push(wheel(delta, Modifiers::NONE));
                frame(&ctx, &mut graph, fresh_input, false);
                assert!(
                    !graph.fitted,
                    "a fresh wheel must still leave automatic mode"
                );
                assert!((graph.pan - fitted_pan - delta).length() < 0.01);
                assert_eq!(graph.zoom, fitted_zoom);
                for _ in 0..40 {
                    frame(&ctx, &mut graph, vec![], false);
                    assert!(
                        (graph.pan - fitted_pan - delta).length() < 0.01,
                        "cancelled wheel tail leaked into the new gesture: {modifiers:?}, immediate={resume_immediately}"
                    );
                    assert_eq!(graph.zoom, fitted_zoom);
                }
            }
        }
    }
}

#[test]
fn history_camera_restore_is_atomic_and_rejects_invalid_coordinates() {
    let mut graph = Graph::default();
    assert!(graph.restore_camera(1.25, Vec2::new(-105.0, 38.0)));
    let camera = graph.camera();
    for (zoom, pan) in [
        (0.0, Vec2::ZERO),
        (-1.0, Vec2::ZERO),
        (f32::NAN, Vec2::ZERO),
        (1.0, Vec2::splat(f32::INFINITY)),
    ] {
        assert!(!graph.restore_camera(zoom, pan));
        assert_eq!(graph.camera(), camera);
    }
    assert!(!graph.fit_pending && !graph.entry_pending && !graph.focus_pending);
    assert!(graph.cancel_wheel);
}

#[test]
fn default_entry_keeps_readable_scale_while_explicit_fit_can_show_the_overview() {
    let mut graph = Graph::default();
    let size = Vec2::new(300.0, 160.0);
    let entry = egui::Rect::from_min_size(egui::pos2(1100.0, 300.0), Vec2::new(300.0, 88.0));
    let whole = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(6000.0, 3000.0));
    graph.readable_entry(size, whole, Some(entry));
    assert!(graph.zoom >= 0.9);
    let screen_entry = graph.screen_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, size), entry);
    assert!(screen_entry.left() >= 0.0 && screen_entry.top() >= 0.0);
    assert!(screen_entry.right() <= size.x);
    assert_eq!(graph.content_label(), "Disasm");
    graph.fit_bounds(size, whole);
    assert!(graph.zoom < 0.1);
}

mod history;
