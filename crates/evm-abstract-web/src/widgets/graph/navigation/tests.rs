use egui::{Context, Event, Modifiers, MouseWheelUnit, Pos2, RawInput, Rect, TouchPhase, Vec2};

use super::super::Graph;
use crate::{app::Selection, tests::report};

fn frame(ctx: &Context, graph: &mut Graph, events: Vec<Event>, sidebar: bool) -> (Rect, f32) {
    let mut canvas = Rect::NOTHING;
    let mut source_offset = 0.0;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))),
            events,
            ..RawInput::default()
        },
        |ui| {
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
            graph.show(ui, &report(), &mut Selection::default());
            let size = graph.viewport.unwrap();
            canvas = Rect::from_min_size(available.right_bottom() - size, size);
        },
    );
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
        let mut graph = Graph::default();
        frame(&ctx, &mut graph, vec![], false);
        let (canvas, _) = frame(&ctx, &mut graph, vec![], false);
        let pointer = if over_node {
            graph
                .screen_rect(canvas, graph.placement.nodes[&0])
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
        assert!(!graph.automatic);
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
        let mut graph = Graph::default();
        frame(&ctx, &mut graph, vec![], false);
        let (canvas, _) = frame(&ctx, &mut graph, vec![], false);
        let pointer = canvas.min + Vec2::new(180.0, 170.0);
        frame(&ctx, &mut graph, vec![Event::PointerMoved(pointer)], false);
        let world = (pointer - canvas.min - graph.pan) / graph.zoom;
        let zoom = graph.zoom;
        frame(&ctx, &mut graph, vec![event], false);
        assert!(graph.zoom > zoom);
        assert!(((pointer - canvas.min - graph.pan) / graph.zoom - world).length() < 0.001);
        assert!(!graph.automatic);
    }
}

#[test]
fn scrolling_a_neighbor_pane_leaves_the_graph_camera_untouched() {
    let ctx = Context::default();
    let mut graph = Graph::default();
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
    assert!(graph.automatic);
}
