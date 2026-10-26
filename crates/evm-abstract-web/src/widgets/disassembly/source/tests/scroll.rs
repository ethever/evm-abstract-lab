use super::*;

#[test]
fn complete_push_operand_remains_horizontally_reachable_in_a_narrow_source_pane() {
    let mut report = crate::tests::report();
    let immediate = format!("0x{}", "fe".repeat(32));
    let instruction =
        &mut std::sync::Arc::make_mut(&mut report.model.programs[0]).blocks[0].instructions[0];
    instruction.name = "PUSH32".into();
    instruction.immediate = Some(immediate.clone());
    let label = super::super::super::copy::instruction(instruction);
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = Focus::source(view, presentation.source(0).unwrap());
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    let mut cache = SourceCache::default();
    let size = egui::vec2(300.0, 350.0);
    render(&context, &mut cache, presentation, &focus, size, vec![]);
    let (output, _) = render(&context, &mut cache, presentation, &focus, size, vec![]);
    assert!(
        text_rect(&output, &label).right() > 300.0,
        "full PUSH32 text must exist beyond the viewport"
    );
    render(
        &context,
        &mut cache,
        presentation,
        &focus,
        size,
        vec![
            egui::Event::PointerMoved(egui::pos2(150.0, 150.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-1000.0, 0.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    for _ in 0..30 {
        render(&context, &mut cache, presentation, &focus, size, vec![]);
    }
    let (output, _) = render(&context, &mut cache, presentation, &focus, size, vec![]);
    assert!(
        (0.0..300.0).contains(&text_rect(&output, &label).right()),
        "the final immediate digits must be reachable by horizontal scroll"
    );
}

fn long_source() -> crate::tests::AnalysisReport {
    let mut report = crate::tests::report();
    let mut instruction = report.model.programs[0].blocks[0].instructions[0].clone();
    instruction.name = "PUSH32".into();
    instruction.opcode = 0x7f;
    instruction.immediate = Some(format!("0x{}", "fe".repeat(32)));
    let source = std::sync::Arc::make_mut(&mut report.model.programs[0]);
    source.blocks.truncate(1);
    source.blocks[0].instructions = (0..1000)
        .map(|pc| {
            let mut instruction = instruction.clone();
            instruction.pc = pc;
            instruction
        })
        .collect();
    report
}

fn scoped_frame(
    context: &Context,
    cache: &mut SourceCache,
    presentation: Presentation<'_>,
    focus: &Focus,
    parent: &str,
    size: egui::Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut output = context.run_ui(
        RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ui| {
            ui.push_id(parent, |ui| {
                cache.show(ui, presentation, focus);
            });
        },
    );
    output.textures_delta.clear();
    output
}

fn first_row(output: &egui::FullOutput) -> (usize, f32) {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => {
                let (pc, _) = text.galley.text().strip_prefix("0x")?.split_once("  ")?;
                Some((usize::from_str_radix(pc, 16).ok()?, text.pos.x))
            }
            _ => None,
        })
        .min_by_key(|(pc, _)| *pc)
        .expect("visible production source row")
}

#[test]
fn manual_scroll_and_offscreen_focus_survive_source_parent_changes_and_resizing() {
    let report = long_source();
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = Focus::source(view, presentation.source(0).unwrap());
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    let mut cache = SourceCache::default();
    let size = egui::vec2(400.0, 450.0);
    for _ in 0..3 {
        scoped_frame(
            &context,
            &mut cache,
            presentation,
            &focus,
            "fullscreen",
            size,
            vec![],
        );
    }
    scoped_frame(
        &context,
        &mut cache,
        presentation,
        &focus,
        "fullscreen",
        size,
        vec![
            egui::Event::PointerMoved(egui::pos2(200.0, 200.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-180.0, -3000.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    for _ in 0..30 {
        scoped_frame(
            &context,
            &mut cache,
            presentation,
            &focus,
            "fullscreen",
            size,
            vec![],
        );
    }
    let before = scoped_frame(
        &context,
        &mut cache,
        presentation,
        &focus,
        "fullscreen",
        size,
        vec![],
    );
    let (pc, x) = first_row(&before);
    assert!(
        pc > 100 && x < -100.0,
        "fixture must scroll on both axes: {pc} {x}"
    );
    for (parent, width) in [
        ("compare_tile", 400.0),
        ("compact_tab", 320.0),
        ("fullscreen", 400.0),
    ] {
        for _ in 0..3 {
            scoped_frame(
                &context,
                &mut cache,
                presentation,
                &focus,
                parent,
                egui::vec2(width, 450.0),
                vec![],
            );
        }
        let output = scoped_frame(
            &context,
            &mut cache,
            presentation,
            &focus,
            parent,
            egui::vec2(width, 450.0),
            vec![],
        );
        let (next_pc, next_x) = first_row(&output);
        assert_eq!(
            next_pc, pc,
            "source manual scroll lost when moved to {parent}"
        );
        assert!(
            (next_x - x).abs() < 1.0,
            "horizontal scroll lost when moved to {parent}: {next_x} vs {x}"
        );
    }
    let selected = SourcePick {
        block: 0,
        pc: Some(999),
    }
    .focus(&focus);
    // ScrollArea clamps a newly requested end-of-content offset after its
    // layout pass. Observe the settled production view, as the pane oracle did.
    for _ in 0..3 {
        scoped_frame(
            &context,
            &mut cache,
            presentation,
            &selected,
            "compare_tile",
            size,
            vec![],
        );
    }
    let output = scoped_frame(
        &context,
        &mut cache,
        presentation,
        &selected,
        "compare_tile",
        size,
        vec![],
    );
    let label = super::super::super::copy::instruction(
        &report.model.programs[0].blocks[0].instructions[999],
    );
    assert!(
        text_rect(&output, &label).intersects(egui::Rect::from_min_size(egui::Pos2::ZERO, size))
    );
    for _ in 0..3 {
        scoped_frame(
            &context,
            &mut cache,
            presentation,
            &selected,
            "compare_tile",
            size,
            vec![],
        );
    }
    let output = scoped_frame(
        &context,
        &mut cache,
        presentation,
        &selected,
        "fullscreen",
        size,
        vec![],
    );
    assert!(
        text_rect(&output, &label).intersects(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        "selected offscreen row must remain visible after reparenting"
    );
}
