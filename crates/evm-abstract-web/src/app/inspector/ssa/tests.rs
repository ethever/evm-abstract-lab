use egui::{Context, Event, FullOutput, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol as api;

use crate::app::{
    Selection,
    inspector::{Inspector, Tab},
};

fn report() -> api::AnalysisReport {
    let mut report = crate::tests::report();
    report.cfg.truncate(1);
    report.ssa.blocks.truncate(1);
    report.edges.clear();
    let instruction = &mut report.ssa.blocks[0].instructions[0];
    instruction.operands = vec![0, 1];
    instruction.operation = api::SsaOperation::Constant("0x3".into());
    report.ssa.value_count = 4;
    report.ssa.values = (0..4)
        .map(|id| api::SsaValue {
            id,
            original_value: 270 + id,
            origin: if id == 3 {
                api::SsaValueOrigin::Instruction(api::SsaInstructionOrigin {
                    state: 0,
                    instruction_index: 0,
                    pc: 0,
                })
            } else {
                api::SsaValueOrigin::Phi(api::SsaPhiOrigin {
                    state: 0,
                    frame: 0,
                    slot: id,
                })
            },
            facts: vec![api::SsaValueFacts {
                status: api::SsaFactStatus::Uncollected,
                scope: api::SsaFactScope::Definition,
                source: api::SsaFactSource::None,
                constant: None,
                value: None,
            }],
            transformation: api::SsaTransformation::Original,
            equivalent: None,
            dependencies: vec![],
        })
        .collect();
    report.ssa.values[3].facts = vec![
        api::SsaValueFacts {
            status: api::SsaFactStatus::Current,
            scope: api::SsaFactScope::State(0),
            source: api::SsaFactSource::Execution,
            constant: None,
            value: Some(report.metadata.environment.call_value.clone()),
        },
        api::SsaValueFacts {
            status: api::SsaFactStatus::Current,
            scope: api::SsaFactScope::Definition,
            source: api::SsaFactSource::Propagation,
            constant: Some("0x3".into()),
            value: None,
        },
    ];
    report.ssa.values[3].transformation = api::SsaTransformation::ConstantFold;
    report.ssa.values[3].dependencies = vec![0, 1];
    report
}

fn render(
    context: &Context,
    events: Vec<Event>,
    contents: impl FnMut(&mut egui::Ui),
) -> FullOutput {
    render_at(context, events, Vec2::new(1400.0, 1800.0), contents)
}

fn render_at(
    context: &Context,
    events: Vec<Event>,
    size: Vec2,
    contents: impl FnMut(&mut egui::Ui),
) -> FullOutput {
    context.global_style_mut(|style| style.animation_time = 0.0);
    let mut output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..RawInput::default()
        },
        contents,
    );
    output.textures_delta.clear();
    output
}

fn texts(output: &FullOutput) -> Vec<(String, Rect)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => {
                let rect = text
                    .galley
                    .rows
                    .iter()
                    .map(|row| {
                        row.rect_without_leading_space()
                            .translate(text.pos.to_vec2())
                    })
                    .reduce(Rect::union)
                    .unwrap_or_else(|| Rect::from_min_size(text.pos, text.galley.size()));
                shape.clip_rect.intersects(rect).then(|| {
                    (
                        evm_abstract_notation::normalize_subscripts(text.galley.text()),
                        rect.intersect(shape.clip_rect),
                    )
                })
            }
            _ => None,
        })
        .collect()
}

fn click(context: &Context, position: Pos2, mut contents: impl FnMut(&mut egui::Ui)) -> FullOutput {
    for pressed in [true, false] {
        render(
            context,
            vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            &mut contents,
        );
    }
    render(context, vec![], contents)
}

#[test]
fn inspector_preserves_observed_and_definition_scopes_with_numeric_properties() {
    let report = report();
    let context = Context::default();
    crate::palette::configure(&context);
    let mut inspector = Inspector::default();
    inspector.select_value(Some(3));
    let mut selection = Selection {
        value: Some(3),
        state: Some(0),
        pc: Some(0),
    };
    let short = render_at(&context, vec![], Vec2::new(1400.0, 210.0), |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let short_text = texts(&short);
    for expected in [
        "Value %3",
        "%3 = CONST 0x3",
        "Go to source",
        "Definition · Current · exact 0x3",
        "State σ0 · Current · Top",
    ] {
        assert!(
            short_text
                .iter()
                .any(|(text, rect)| { text == expected && rect.bottom() <= 210.0 }),
            "missing short-panel summary {expected}: {short_text:?}"
        );
    }
    assert!(
        !short_text
            .iter()
            .any(|(text, _)| text == "%273" || text == "ConstantFold")
    );
    render(&context, vec![], |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let output = render(&context, vec![], |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let source = texts(&output)
        .into_iter()
        .find(|(text, _)| text == "Source and derivation")
        .unwrap()
        .1
        .center();
    let output = click(&context, source, |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let visible = texts(&output)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>();
    for expected in [
        "Value %3",
        "%273",
        "%3 = CONST 0x3",
        "ConstantFold",
        "State σ0 · Current",
        "Definition · Current",
        "Execution",
        "Propagation",
        "Known zero bits",
        "Known one bits",
        "Unsigned range",
        "Unrestricted",
        "0x3",
        "Applies within the recorded native-state context σ0.",
    ] {
        assert!(
            visible.iter().any(|text| text == expected),
            "missing {expected}: {visible:?}"
        );
    }
    let summary = inspector.accessible_summary(&report, selection);
    for expected in [
        "SSA value %3",
        "Current · State σ₀",
        "Current · Definition",
        "CONST 0x3",
    ] {
        assert!(summary.contains(expected), "missing {expected}: {summary}");
    }
    assert_eq!(inspector.tab, Tab::SsaValue);
}

#[test]
fn availability_is_distinct_from_a_collected_top_value() {
    let mut report = report();
    let context = Context::default();
    crate::palette::configure(&context);
    let mut inspector = Inspector::default();
    inspector.select_value(Some(3));
    let mut selection = Selection {
        value: Some(3),
        ..Selection::default()
    };
    for (status, expected) in [
        (
            api::SsaFactStatus::Uncollected,
            "No numeric fact was collected for this scope.",
        ),
        (
            api::SsaFactStatus::Stale,
            "Execution receipt is stale; this fact is not used to justify normalization.",
        ),
        (
            api::SsaFactStatus::Unexecuted,
            "This state has no execution receipt.",
        ),
    ] {
        report.ssa.values[3].facts = vec![api::SsaValueFacts {
            status,
            scope: api::SsaFactScope::State(0),
            source: api::SsaFactSource::None,
            constant: None,
            value: None,
        }];
        let output = render(&context, vec![], |ui| {
            inspector.show(ui, &report, &mut selection, None)
        });
        let visible = texts(&output);
        assert!(visible.iter().any(|(text, _)| text == expected));
        assert!(!visible.iter().any(|(text, _)| text == "Unrestricted"));
        assert!(
            inspector
                .accessible_summary(&report, selection)
                .contains(&format!("{status:?}"))
        );
    }
    report.ssa.values[0].facts = vec![api::SsaValueFacts {
        status: api::SsaFactStatus::Stale,
        scope: api::SsaFactScope::State(0),
        source: api::SsaFactSource::Entry,
        constant: None,
        value: Some(report.metadata.environment.call_value.clone()),
    }];
    selection.value = Some(0);
    let output = render(&context, vec![], |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let visible = texts(&output);
    assert!(visible.iter().any(|(text, _)| text == "Entry"));
    assert!(visible.iter().any(|(text, _)| text == "Unrestricted"));
    assert!(visible.iter().any(|(text, _)| {
        text == "Execution receipt is stale; this fact is not used to justify normalization."
    }));
    assert!(
        !visible
            .iter()
            .any(|(text, _)| text.contains("Historical fact"))
    );
}

#[test]
fn equality_navigation_keeps_reading_coordinates_and_source_navigation_uses_native_origin() {
    let mut report = report();
    report.ssa.values[3].equivalent = Some(0);
    let context = Context::default();
    crate::palette::configure(&context);
    let mut inspector = Inspector::default();
    inspector.select_value(Some(3));
    let mut selection = Selection {
        value: Some(3),
        state: Some(91),
        pc: Some(400),
    };
    render(&context, vec![], |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let output = render(&context, vec![], |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let source_details = texts(&output)
        .into_iter()
        .find(|(text, _)| text == "Source and derivation")
        .unwrap()
        .1
        .center();
    let output = click(&context, source_details, |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let equal = texts(&output)
        .into_iter()
        .find(|(text, _)| text == "%0")
        .unwrap()
        .1
        .center();
    for pressed in [true, false] {
        render(
            &context,
            vec![
                Event::PointerMoved(equal),
                Event::PointerButton {
                    pos: equal,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            |ui| inspector.show(ui, &report, &mut selection, None),
        );
    }
    assert_eq!(
        selection,
        Selection {
            value: Some(0),
            state: Some(91),
            pc: Some(400)
        },
        "equal link position {equal:?}"
    );
    selection.value = Some(3);
    let output = render(&context, vec![], |ui| {
        inspector.show(ui, &report, &mut selection, None)
    });
    let source = texts(&output)
        .into_iter()
        .find(|(text, _)| text == "Go to source")
        .unwrap()
        .1
        .center();
    for pressed in [true, false] {
        render(
            &context,
            vec![
                Event::PointerMoved(source),
                Event::PointerButton {
                    pos: source,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            |ui| inspector.show(ui, &report, &mut selection, None),
        );
    }
    assert_eq!(
        selection,
        Selection {
            value: Some(3),
            state: Some(0),
            pc: Some(0)
        }
    );
    inspector.reset_report();
    assert_eq!(inspector.last_value, None);
}
