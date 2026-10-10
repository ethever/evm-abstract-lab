use egui::{Context, RawInput};

use super::SsaCache;
use crate::{
    app::Selection,
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
        state: Some(0),
        pc: None,
    };
    let mut previous = selection;
    let mut row_storage = None;
    let mut widths = Vec::new();
    for state in [0, 2, 1] {
        selection = Selection {
            state: Some(state),
            pc: Some(5),
        };
        let mut output = context.run_ui(RawInput::default(), |ui| {
            widths.push(natural_width_cached(
                ui,
                &report,
                Some(0),
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
                &report,
                Some(0),
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
    cache.prepare(&report, Some(0), Selection::default());
    assert_eq!(headers(&cache), [0]);
    cache.prepare(&report, Some(9), Selection::default());
    assert_eq!(headers(&cache), [1]);
    cache.prepare(
        &report,
        None,
        Selection {
            state: Some(2),
            pc: None,
        },
    );
    assert_eq!(headers(&cache), [2]);
    cache.prepare(
        &report,
        None,
        Selection {
            state: Some(0),
            pc: None,
        },
    );
    assert!(cache.rows().is_empty());
    assert_eq!(cache.columns(), 0);
    cache.prepare(&report, Some(0), Selection::default());
    assert_eq!(headers(&cache), [0]);
}

#[test]
fn accepting_a_new_report_clears_both_rows_and_width_even_with_identical_program_ids() {
    let mut report = crate::tests::report();
    let mut cache = SsaCache::default();
    cache.prepare(&report, Some(0), Selection::default());
    let original_width = cache.columns();
    // A replacement can have identical graph IDs but different operands or
    // coverage, so its owner must explicitly invalidate rather than hash IDs.
    report.ssa.blocks[0].instructions[0].name = "NEW_RECEIPT_".repeat(original_width + 1);
    cache.clear();
    assert!(cache.rows().is_empty());
    assert_eq!(cache.columns(), 0);
    cache.prepare(&report, Some(0), Selection::default());
    assert_eq!(headers(&cache), [0, 1, 2]);
    assert!(
        cache
            .rows()
            .iter()
            .any(|row| row.plain_text().contains("NEW_RECEIPT_"))
    );
    assert!(cache.columns() > original_width);
}
