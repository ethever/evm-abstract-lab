//! Canonical indexed source rows and source-only layout measurements.
mod copy;
mod source;
#[cfg(test)]
mod tests;
mod tooltip;

pub(super) use copy::reference as source_reference;
pub(crate) use source::SourceCache;

pub(super) fn location_label(location: evm_abstract_protocol::ProcessLocation) -> String {
    crate::app::minimum::location_label(location)
}

/// Measure decoded text independently of state count or receipt membership.
/// The responsive layout caches this value by accepted view/source preference.
pub(crate) fn natural_width(
    ui: &egui::Ui,
    report: &crate::report::ReportView<'_>,
    program: Option<usize>,
    _selection: crate::app::Selection,
) -> f32 {
    let Some(program) =
        program.and_then(|id| report.programs.iter().find(|program| program.id == id))
    else {
        return 200.0;
    };
    let columns = program
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .map(|instruction| copy::instruction(instruction).chars().count())
        .max()
        .unwrap_or(20);
    let glyph = ui
        .painter()
        .layout_no_wrap(
            "0".into(),
            egui::TextStyle::Monospace.resolve(ui.style()),
            crate::palette::TEXT,
        )
        .size()
        .x;
    // Reference-copy controls remain reachable beside full PUSH immediates.
    (columns as f32 * glyph + 76.0).max(200.0)
}
