use egui::{Context, RawInput};

use super::SsaCache;
use crate::{
    app::Selection,
    ssa_display::SsaDisplay,
    widgets::ssa::{Row, natural_width_cached, ssa_cached},
};

fn headers(cache: &SsaCache) -> Vec<usize> {
    cache
        .rows()
        .iter()
        .filter(|row| row.header)
        .map(|row| row.target.unwrap().state.unwrap())
        .collect()
}

#[test]
fn width_and_table_share_rows_across_frames_and_native_selection_changes() {
    let report = crate::tests::report();
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    let mut cache = SsaCache::default();
    let mut selection = Selection {
        value: None,
        state: Some(0),
        pc: None,
    };
    let mut previous = selection;
    let mut row_storage = None;
    let mut widths = Vec::new();
    for state in [0, 2, 1] {
        selection = Selection {
            value: None,
            state: Some(state),
            pc: Some(5),
        };
        let mut output = context.run_ui(RawInput::default(), |ui| {
            widths.push(natural_width_cached(
                ui,
                &(report).view(),
                Some(0),
                SsaDisplay::Compact,
                selection,
                &mut cache,
            ));
            let pointer = cache.rows.as_ptr();
            if let Some(expected) = row_storage {
                assert_eq!(
                    pointer, expected,
                    "same program must keep the original row storage"
                );
            }
            row_storage = Some(pointer);
            ssa_cached(
                ui,
                &(report).view(),
                Some(0),
                SsaDisplay::Compact,
                &mut selection,
                &mut previous,
                &mut cache,
            );
            assert_eq!(
                cache.rows.as_ptr(),
                pointer,
                "painting must reuse width measurement's rows"
            );
        });
        output.textures_delta.clear();
    }
    assert_eq!(headers(&cache), [0, 1, 2]);
    assert!(widths.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(
        cache.columns(),
        cache.rows().iter().map(Row::columns).max().unwrap()
    );
}

#[test]
fn scope_changes_rebuild_native_rows_and_no_code_scope_tracks_the_selected_state() {
    let mut report = crate::tests::report();
    report.cfg[1].program = Some(9);
    report.cfg[2].program = None;
    let mut cache = SsaCache::default();
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        Selection::default(),
    );
    assert_eq!(headers(&cache), [0]);
    cache.prepare(
        &(report).view(),
        Some(9),
        SsaDisplay::Compact,
        Selection::default(),
    );
    assert_eq!(headers(&cache), [1]);
    cache.prepare(
        &(report).view(),
        None,
        SsaDisplay::Compact,
        Selection {
            value: None,
            state: Some(2),
            pc: None,
        },
    );
    assert_eq!(headers(&cache), [2]);
    cache.prepare(
        &(report).view(),
        None,
        SsaDisplay::Compact,
        Selection {
            value: None,
            state: Some(0),
            pc: None,
        },
    );
    assert!(cache.rows().is_empty());
    assert_eq!(cache.columns(), 0);
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        Selection::default(),
    );
    assert_eq!(headers(&cache), [0]);
}

#[test]
fn accepting_a_new_report_clears_both_rows_and_width_even_with_identical_program_ids() {
    let mut report = crate::tests::report();
    let mut cache = SsaCache::default();
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        Selection::default(),
    );
    let original_width = cache.columns();
    // A replacement can have identical graph IDs but different operands or
    // coverage, so its owner must explicitly invalidate rather than hash IDs.
    report.ssa.blocks[0].instructions[0].name = "NEW_RECEIPT_".repeat(original_width + 1);
    crate::tests::refresh_operation(&mut report.ssa.blocks[0].instructions[0]);
    cache.activate(crate::app::model::ReportViewId::fixture(
        2,
        evm_abstract_protocol::AnalysisNamespace::Native,
    ));
    assert!(cache.rows().is_empty());
    assert_eq!(cache.columns(), 0);
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        Selection::default(),
    );
    assert_eq!(headers(&cache), [0, 1, 2]);
    assert!(
        cache
            .rows()
            .iter()
            .any(|row| row.plain_text().contains("NEW_RECEIPT_"))
    );
    assert!(cache.columns() > original_width);
}

#[test]
fn display_and_hidden_selection_rebuild_rows_while_ordinary_selection_reuses_storage() {
    let mut report = crate::tests::report();
    let mut instruction = report.ssa.blocks[0].instructions[0].clone();
    instruction.opcode = 0x80;
    instruction.name = "DUP1".into();
    crate::tests::refresh_operation(&mut instruction);
    instruction.pc = 30;
    instruction.progress = evm_abstract_protocol::InstructionProgress::Completed;
    instruction.fault = false;
    instruction.results.clear();
    let mut swap = instruction.clone();
    swap.opcode = 0x90;
    swap.name = "SWAP1".into();
    crate::tests::refresh_operation(&mut swap);
    swap.pc = 31;
    let mut visible = instruction.clone();
    visible.opcode = 0x52;
    visible.name = "MSTORE".into();
    crate::tests::refresh_operation(&mut visible);
    visible.pc = 32;
    report.ssa.blocks[0].coverage = evm_abstract_protocol::BlockCoverage::Current;
    report.ssa.blocks[0].instructions = vec![instruction, swap, visible];
    let selection = |pc| Selection {
        value: None,
        state: Some(0),
        pc,
    };
    let has_pc = |cache: &SsaCache, pc| {
        cache
            .rows()
            .iter()
            .any(|row| row.target == Some(selection(Some(pc))))
    };
    let mut cache = SsaCache::default();
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        selection(None),
    );
    let compact_count = cache.rows().len();
    let compact_storage = cache.rows().as_ptr();
    assert!(!has_pc(&cache, 30));
    assert!(!has_pc(&cache, 31));
    assert!(has_pc(&cache, 32));
    for selected in [
        selection(Some(32)),
        Selection {
            value: None,
            state: Some(1),
            pc: Some(5),
        },
        Selection::default(),
    ] {
        cache.prepare(&(report).view(), Some(0), SsaDisplay::Compact, selected);
        assert_eq!(cache.rows().as_ptr(), compact_storage);
    }
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        selection(Some(30)),
    );
    assert_ne!(cache.rows().as_ptr(), compact_storage);
    assert_eq!(cache.rows().len(), compact_count + 1);
    assert!(has_pc(&cache, 30));
    assert!(!has_pc(&cache, 31));
    let revealed_storage = cache.rows().as_ptr();
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        selection(Some(30)),
    );
    assert_eq!(cache.rows().as_ptr(), revealed_storage);
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        selection(Some(31)),
    );
    assert_ne!(cache.rows().as_ptr(), revealed_storage);
    assert!(!has_pc(&cache, 30));
    assert!(has_pc(&cache, 31));
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Full,
        selection(Some(31)),
    );
    assert_eq!(cache.rows().len(), compact_count + 2);
    assert!(has_pc(&cache, 30));
    assert!(has_pc(&cache, 31));
    let full_storage = cache.rows().as_ptr();
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Full,
        selection(Some(30)),
    );
    assert_eq!(cache.rows().as_ptr(), full_storage);
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        selection(Some(30)),
    );
    assert_ne!(cache.rows().as_ptr(), full_storage);
    assert_eq!(cache.rows().len(), compact_count + 1);
    assert!(has_pc(&cache, 30));
    assert!(!has_pc(&cache, 31));
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        selection(Some(32)),
    );
    assert_eq!(cache.rows().len(), compact_count);
    let compact_storage = cache.rows().as_ptr();
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        selection(None),
    );
    assert_eq!(cache.rows().as_ptr(), compact_storage);
}

#[test]
fn a_replacement_report_clears_the_hidden_instruction_index() {
    let mut report = crate::tests::report();
    let instruction = &mut report.ssa.blocks[0].instructions[0];
    instruction.opcode = 0x50;
    instruction.name = "POP".into();
    crate::tests::refresh_operation(instruction);
    instruction.progress = evm_abstract_protocol::InstructionProgress::Completed;
    instruction.fault = false;
    report.ssa.blocks[0].coverage = evm_abstract_protocol::BlockCoverage::Current;
    let mut cache = SsaCache::default();
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        Selection::default(),
    );
    assert!(!cache.hidden.is_empty());
    report.ssa.blocks[0].instructions[0].opcode = 0x52;
    report.ssa.blocks[0].instructions[0].name = "MSTORE".into();
    crate::tests::refresh_operation(&mut report.ssa.blocks[0].instructions[0]);
    cache.activate(crate::app::model::ReportViewId::fixture(
        2,
        evm_abstract_protocol::AnalysisNamespace::Native,
    ));
    assert!(cache.hidden.is_empty());
    cache.prepare(
        &(report).view(),
        Some(0),
        SsaDisplay::Compact,
        Selection::default(),
    );
    assert!(cache.hidden.is_empty());
    assert!(
        cache
            .rows()
            .iter()
            .any(|row| row.plain_text().contains("MSTORE"))
    );
}
