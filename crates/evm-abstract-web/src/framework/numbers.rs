//! Lossless decimal drafts for typed integer settings. Invalid text stays in
//! the editor; callers must gate submission on aggregate validity.

use std::{collections::BTreeMap, str::FromStr};

use egui::Ui;

use crate::palette;

#[derive(Default)]
pub(crate) struct NumberInputs {
    drafts: BTreeMap<&'static str, NumberDraft>,
}

struct NumberDraft {
    text: String,
    valid: bool,
}

impl NumberInputs {
    pub(crate) fn valid(&self) -> bool {
        self.drafts.values().all(|draft| draft.valid)
    }

    // DragValue converts integer edits through f64, losing values above 2^53.
    // Keep decimal text until the concrete integer type accepts it exactly.
    pub(crate) fn number<T: FromStr + ToString>(
        &mut self,
        ui: &mut Ui,
        label: &'static str,
        value: &mut T,
    ) {
        ui.label(label);
        let draft = self.drafts.entry(label).or_insert_with(|| NumberDraft {
            text: value.to_string(),
            valid: true,
        });
        let response = super::scoped_editor(
            ui,
            label,
            &mut draft.text,
            "Unsigned whole number",
            180.0_f32.min(ui.available_width()),
        );
        if response.changed() {
            match draft.text.parse() {
                Ok(parsed) => {
                    *value = parsed;
                    draft.valid = true;
                }
                Err(_) => draft.valid = false,
            }
        }
        ui.end_row();
        if !draft.valid {
            ui.label("");
            ui.colored_label(palette::ERROR, "Enter a supported unsigned whole number");
            ui.end_row();
        }
    }
}
