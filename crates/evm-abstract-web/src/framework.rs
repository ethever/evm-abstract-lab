//! Minimal adapters for egui text buffers, tile behaviors and browser callbacks.
//! Application state and custom analysis widgets use static dispatch.

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
pub use browser::start;

/// Keep pasted multi-line programs inside the input budget instead of growing
/// the top panel until it covers the analysis panes.
pub(crate) fn bytecode_editor(ui: &mut egui::Ui, bytecode: &mut String, height: f32) {
    egui::ScrollArea::vertical()
        .id_salt("bytecode_scroll")
        .max_height(height)
        // Auto-sized top panels initially offer zero remaining height. Give
        // the scroll viewport its budget before shrinking to a short input.
        .min_scrolled_height(height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(bytecode)
                    .id(egui::Id::new("runtime_bytecode"))
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(1)
                    .hint_text("Runtime bytecode: 0x60806040…"),
            );
        });
}

/// egui_tiles owns the dynamic behavior call; application widgets keep a
/// concrete behavior type and do not inherit that framework requirement.
pub(crate) fn pane_tree<Pane, B: egui_tiles::Behavior<Pane>>(
    ui: &mut egui::Ui,
    tree: &mut egui_tiles::Tree<Pane>,
    behavior: &mut B,
) {
    tree.ui(behavior, ui);
}

/// Text-buffer type erasure remains inside the framework boundary.
pub(crate) fn line_editor(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    hint: &str,
) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .id(egui::Id::new(id))
            .desired_width(ui.available_width())
            .hint_text(hint),
    )
}

/// Repeated rows use a parent-scoped ID instead of a global field identity.
pub(crate) fn scoped_editor(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    hint: &str,
    width: f32,
) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .id(ui.make_persistent_id(id))
            .desired_width(width)
            .hint_text(hint),
    )
}
