use super::{SourceCache, SourcePick};
use crate::app::model::{
    ReportViewId,
    navigation::{EvidenceFocus, Focus},
    presentation::{Presentation, ReportIndex},
};
use egui::{Context, RawInput};
use evm_abstract_protocol::AnalysisNamespace;

#[test]
fn source_picks_keep_real_block_ids_without_claiming_a_native_state() {
    let mut report = crate::tests::report();
    std::sync::Arc::make_mut(&mut report.model.programs[0]).blocks[0].id = 91;
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let current = Focus::source(view, presentation.source(0).unwrap());
    let selected = SourcePick {
        block: 91,
        pc: Some(0),
    }
    .focus(&current);
    assert!(presentation.validate(&selected).is_ok());
    assert!(matches!(
        selected.evidence,
        EvidenceFocus::Source {
            block: Some(91),
            pc: Some(0)
        }
    ));
    assert_eq!(selected.context, None);
    assert_eq!(selected.graph_selection(), None);
    let wrong = SourcePick {
        block: 0,
        pc: Some(0),
    }
    .focus(&current);
    assert!(presentation.validate(&wrong).is_err());
    let wrong_pc = SourcePick {
        block: 91,
        pc: Some(999_999),
    }
    .focus(&current);
    assert!(presentation.validate(&wrong_pc).is_err());
    assert!(
        presentation
            .validate(
                &SourcePick {
                    block: 91,
                    pc: None
                }
                .focus(&current)
            )
            .is_ok()
    );
}

#[test]
fn canonical_source_virtualizes_large_inputs_and_rejects_stale_focus() {
    let mut report = crate::tests::report();
    let mut block = report.model.programs[0].blocks[0].clone();
    block.instructions.truncate(1);
    let mut blocks = Vec::new();
    for index in 0..10_000 {
        let mut block = block.clone();
        block.id = index * 3;
        block.start_pc = index * 5;
        block.instructions[0].pc = block.start_pc;
        blocks.push(block);
    }
    std::sync::Arc::make_mut(&mut report.model.programs[0]).blocks = blocks;
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = Focus::source(view, presentation.source(0).unwrap());
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    let mut cache = SourceCache::default();
    let mut output = context.run_ui(
        RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 480.0),
            )),
            ..Default::default()
        },
        |ui| {
            assert!(cache.show(ui, presentation, &focus).is_none());
        },
    );
    output.textures_delta.clear();
    let rows = crate::tests::painted_text(&output)
        .iter()
        .filter(|(text, _)| text.starts_with("Block 0x"))
        .count();
    assert!(rows > 0 && rows < 100, "visible block labels: {rows}");
    let mut stale = focus.clone();
    stale.view = ReportViewId::fixture(2, AnalysisNamespace::Native);
    let mut output = context.run_ui(
        RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 480.0),
            )),
            ..Default::default()
        },
        |ui| {
            assert!(cache.show(ui, presentation, &stale).is_none());
        },
    );
    output.textures_delta.clear();
    assert!(
        crate::tests::painted_text(&output)
            .iter()
            .any(|(text, _)| text.contains("earlier result"))
    );
    assert!(
        !crate::tests::painted_text(&output)
            .iter()
            .any(|(text, _)| text.starts_with("Block 0x"))
    );
}

#[test]
fn clipboard_code_and_reference_preserve_full_operands_and_source_namespace() {
    let mut report = crate::tests::report();
    let immediate = format!("0x{}", "fe".repeat(32));
    let source = std::sync::Arc::make_mut(&mut report.model.programs[0]);
    source.blocks[0].instructions[0].immediate = Some(immediate.clone());
    let code = super::super::copy::code(source);
    assert!(code.contains(&immediate));
    let view = ReportViewId::fixture(17, AnalysisNamespace::Native);
    let reference = super::super::copy::source_reference(source, view, Some(0), Some(91));
    assert!(reference.contains(&source.code_hash));
    assert!(reference.contains("Native"));
    assert!(reference.contains("PC: 0x0"));
    assert!(reference.contains("State: 91"));
    assert!(reference.contains("17"));
}

#[test]
fn native_pc_links_require_the_selected_states_current_receipt() {
    let mut report = crate::tests::report();
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = presentation
        .focus_context(crate::app::model::presentation::ContextTarget {
            view,
            context: crate::app::model::navigation::ContextFocus::Graph { state: 0 },
        })
        .unwrap();
    let pick = SourcePick {
        block: 0,
        pc: Some(0),
    };
    let native = pick.context_focus(presentation, &focus);
    assert_eq!(native.graph_selection().unwrap().state, Some(0));
    assert_eq!(native.graph_selection().unwrap().pc, Some(0));
    let unrecorded = SourcePick {
        block: 0,
        pc: Some(999_999),
    }
    .context_focus(presentation, &focus);
    assert!(matches!(unrecorded.evidence, EvidenceFocus::Source { .. }));
    report.ssa.blocks[0].coverage = evm_abstract_protocol::BlockCoverage::Stale;
    let mut stale_index = ReportIndex::default();
    stale_index.update(&report.model, view, None, None);
    let stale = Presentation::new(&report.model, &stale_index, view, None);
    assert!(matches!(
        pick.context_focus(stale, &focus).evidence,
        EvidenceFocus::Source { .. }
    ));
}

#[test]
fn canonical_hover_uses_only_the_selected_contexts_current_instruction_receipt() {
    use evm_abstract_protocol::BlockCoverage;
    for (state, coverage, observed) in [
        (Some(0), BlockCoverage::Current, true),
        (Some(1), BlockCoverage::Current, false),
        (None, BlockCoverage::Current, false),
        (Some(0), BlockCoverage::Stale, false),
    ] {
        let mut report = crate::tests::report();
        report.ssa.blocks[0].coverage = coverage;
        let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
        let mut index = ReportIndex::default();
        index.update(&report.model, view, None, None);
        let presentation = Presentation::new(&report.model, &index, view, None);
        let focus = state.map_or_else(
            || Focus::source(view, presentation.source(0).unwrap()),
            |state| {
                presentation
                    .focus_context(crate::app::model::presentation::ContextTarget {
                        view,
                        context: crate::app::model::navigation::ContextFocus::Graph { state },
                    })
                    .unwrap()
            },
        );
        let context = Context::default();
        crate::palette::configure(&context);
        context.style_mut_of(egui::Theme::Dark, |style| {
            style.interaction.tooltip_delay = 0.0
        });
        let mut cache = SourceCache::default();
        let mut frame = |time, events| {
            let mut output = context.run_ui(
                RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 480.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    cache.show(ui, presentation, &focus);
                },
            );
            output.textures_delta.clear();
            output
        };
        frame(0.0, vec![]);
        let output = frame(0.1, vec![]);
        let label =
            super::super::copy::instruction(&report.model.programs[0].blocks[0].instructions[0]);
        let pos = crate::tests::painted_text(&output)
            .iter()
            .find(|(text, _)| text == &label)
            .unwrap()
            .1
            + egui::vec2(8.0, 6.0);
        frame(0.2, vec![egui::Event::PointerMoved(pos)]);
        frame(1.0, vec![]);
        let output = frame(1.1, vec![]);
        let text = crate::tests::painted_text(&output)
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
            .join("\n");
        if observed {
            assert!(
                text.contains("Observed in current execution evidence"),
                "{text}"
            );
        } else {
            assert!(
                text.contains("Decoded source; no current execution receipt"),
                "{text}"
            );
            assert!(
                !text.contains("Observed in current execution evidence"),
                "{text}"
            );
        }
    }
}
