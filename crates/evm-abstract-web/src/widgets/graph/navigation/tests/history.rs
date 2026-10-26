//! Exercise the first renderer frame after same-program reentry navigation,
//! including scope/layout invalidation that happens after camera restoration.
use crate::{
    app::Selection,
    tests::AnalysisReport,
    widgets::graph::{Graph, GraphMode},
};
use egui::{Context, Pos2, RawInput, Rect, Vec2};

fn frame(ctx: &Context, graph: &mut Graph, report: &AnalysisReport, selection: &mut Selection) {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 760.0))),
            ..Default::default()
        },
        |ui| {
            let _ = graph.show_scoped(ui, &report.view(), selection);
        },
    );
    output.textures_delta.clear();
}

#[test]
fn same_program_owner_static_reentry_back_restores_camera_on_its_first_render() {
    let mut report = crate::tests::report();
    // Display counterexample: the code source is identical, but the second
    // recorded invocation has a different owner/caller and is read only.
    let owner = "0x2222222222222222222222222222222222222222";
    report.cfg[1].storage_address = Some(owner.into());
    let active = &mut report.states[1].entry.frames[0];
    active.storage_address = Some(owner.into());
    active.address_value = Some(evm_abstract_protocol::AddressValue::Concrete(owner.into()));
    active.caller = Some(evm_abstract_protocol::AddressValue::Concrete(
        "0x3333333333333333333333333333333333333333".into(),
    ));
    active.is_static = true;
    for mode in [GraphMode::Blocks, GraphMode::Local] {
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        let mut graph = Graph::default();
        graph.set_mode(mode);
        let mut selection = Selection {
            state: Some(0),
            value: None,
            pc: None,
        };
        frame(&ctx, &mut graph, &report, &mut selection);
        graph.restore_camera(1.15, Vec2::new(-76.0, 31.0));
        frame(&ctx, &mut graph, &report, &mut selection);
        let camera = graph.camera();
        let geometry = graph.scene.placement.nodes.clone();
        selection.state = Some(1);
        frame(&ctx, &mut graph, &report, &mut selection);
        assert_eq!(graph.scene.native_context, Some(1));
        assert!(!graph.scene.display.state_to_node.contains_key(&0));
        selection.state = Some(0);
        assert!(graph.restore_camera(camera.0, camera.1));
        frame(&ctx, &mut graph, &report, &mut selection);
        assert_eq!(
            graph.camera(),
            camera,
            "same-program {mode:?} invalidation overwrote Back camera"
        );
        assert_eq!(graph.scene.native_context, Some(0));
        assert_eq!(graph.scene.placement.nodes, geometry);
        assert!(!graph.scene.display.state_to_node.contains_key(&1));
    }
}
