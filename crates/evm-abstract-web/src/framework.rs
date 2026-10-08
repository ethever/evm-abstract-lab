//! Minimal adapters for egui's dynamic text buffer and eframe's browser callbacks.
//! Application state and custom analysis widgets use static dispatch.

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
pub use browser::start;

pub(crate) fn bytecode_editor(ui: &mut egui::Ui, bytecode: &mut String) {
    ui.add(
        egui::TextEdit::multiline(bytecode)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY)
            .desired_rows(2)
            .hint_text("0x60806040…"),
    );
}
