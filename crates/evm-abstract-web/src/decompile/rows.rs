//! The same measured stride drives virtualization and navigation. Every row
//! reserves this height; clipping determines when navigation has arrived.
pub(super) const HEIGHT: f32 = 22.0;

#[cfg(test)]
pub(super) fn begin_frame(context: &egui::Context) {
    context.data_mut(|data| data.insert_temp(egui::Id::new("decompile_prepared_rows"), 0_usize));
}

#[cfg(test)]
pub(super) fn prepared(context: &egui::Context) -> usize {
    context.data(|data| {
        data.get_temp(egui::Id::new("decompile_prepared_rows"))
            .unwrap_or(0)
    })
}

pub(super) fn offset(ui: &egui::Ui, position: usize) -> f32 {
    position as f32 * (HEIGHT + ui.spacing().item_spacing.y)
}

pub(super) fn visible(ui: &egui::Ui, response: &egui::Response) -> bool {
    let clip = ui.clip_rect();
    // Long IR text may require horizontal scrolling; arrival is established
    // when its row and identifying prefix are exposed in the vertical pane.
    clip.intersects(response.rect)
        && clip.top() <= response.rect.top()
        && response.rect.bottom() <= clip.bottom()
}

pub(super) fn row<R>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    show: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    #[cfg(test)]
    ui.ctx().data_mut(|data| {
        *data.get_temp_mut_or_default::<usize>(egui::Id::new("decompile_prepared_rows")) += 1
    });
    ui.push_id(id, |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_height(HEIGHT);
                show(ui)
            },
        )
        .inner
    })
    .inner
}
