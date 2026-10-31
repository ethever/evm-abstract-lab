//! Minimal adapters for egui text buffers, tile behaviors and browser callbacks.
//! Application state and custom analysis widgets use static dispatch.

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
pub use browser::start;

#[cfg(test)]
mod contact;
#[cfg(test)]
mod editors;

mod submit;
pub(crate) use submit::SubmitKeyboard;

/// Preserve the native disclosure while exposing its actual header response.
pub(crate) fn section<R>(
    ui: &mut egui::Ui,
    label: &str,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::CollapsingResponse<R> {
    let section = egui::CollapsingHeader::new(label).show(ui, contents);
    crate::controls::observe(
        ui,
        &section.header_response,
        label,
        "section",
        Some(section.body_response.is_some()),
    );
    section
}

pub(crate) fn checkbox(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    let response = ui.checkbox(value, label);
    crate::controls::observe(ui, &response, label, "checkbox", Some(*value));
    response
}

/// Keep pasted multi-line programs inside the input budget instead of growing
/// the top panel until it covers the analysis panes.
#[derive(Clone, Copy, Debug, Hash)]
pub(crate) enum BytecodeEditor {
    Analysis,
    #[cfg(any(target_arch = "wasm32", test))]
    Decompiler,
}

impl BytecodeEditor {
    pub(crate) fn id(self) -> egui::Id {
        egui::Id::new(("runtime_bytecode", self))
    }

    /// Explicit replacement starts a new edit session. Ordinary close/reopen
    /// preserves this editor's cursor and undo history.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn replace(self, context: &egui::Context) {
        egui::text_edit::TextEditState::default().store(context, self.id());
    }
}

pub(crate) fn bytecode_editor(
    ui: &mut egui::Ui,
    editor: BytecodeEditor,
    bytecode: &mut String,
    height: f32,
) {
    egui::ScrollArea::vertical()
        .id_salt("bytecode_scroll")
        .max_height(height)
        // Auto-sized top panels initially offer zero remaining height. Give
        // the scroll viewport its budget before shrinking to a short input.
        .min_scrolled_height(height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let response = ui.add(
                egui::TextEdit::multiline(bytecode)
                    .id(editor.id())
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(1)
                    .hint_text("Runtime bytecode: 0x60806040…"),
            );
            crate::controls::observe(ui, &response, "Runtime bytecode", "text", None);
            // Opt-in evidence reads the actual buffer after native text events;
            // it does not provide another activation or mutation path.
            crate::controls::observe(ui, &response, bytecode, "bytecode-value", None);
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
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .id(egui::Id::new(id))
            .desired_width(ui.available_width())
            .hint_text(crate::notation::widget(ui, hint)),
    );
    crate::controls::observe(ui, &response, id, "text", None);
    response
}

/// Repeated rows use a parent-scoped ID instead of a global field identity.
pub(crate) fn scoped_editor(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    hint: &str,
    width: f32,
) -> egui::Response {
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .id(ui.make_persistent_id(id))
            .desired_width(width)
            .hint_text(crate::notation::widget(ui, hint)),
    );
    crate::controls::observe(ui, &response, id, "text", None);
    response
}
