//! Submission never doubles as an IME commit or a held-key repeat. Read pending
//! events before TextEdit consumes them; composition can span several frames.

#[derive(Default)]
pub(crate) struct SubmitKeyboard {
    composing: bool,
}

impl SubmitKeyboard {
    pub(crate) fn reset(&mut self) {
        self.composing = false;
    }

    pub(crate) fn submit(&mut self, ctx: &egui::Context) -> bool {
        let accepted = ctx.input(|input| {
            let mut ime_frame = self.composing;
            let mut submit = false;
            for event in &input.events {
                match event {
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                        self.composing = !text.is_empty();
                        ime_frame = true;
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                        self.composing = false;
                        ime_frame = true;
                    }
                    egui::Event::WindowFocused(false) => self.composing = false,
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } if modifiers.ctrl
                        && !modifiers.alt
                        && !modifiers.shift
                        && !modifiers.mac_cmd =>
                    {
                        submit = true
                    }
                    _ => {}
                }
            }
            submit && !ime_frame
        });
        // Also consume blocked chords, so text editing cannot insert a newline
        // for a submission chord and a later widget cannot reinterpret it.
        ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Enter));
        accepted
    }
}
