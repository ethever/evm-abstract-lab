//! Opt-in evidence from actual egui responses, never a second activation path.
//! Browser checks still send pointer/keyboard input and verify painted output.

use egui::{Context, Id, Response, Ui};

#[derive(Clone, Default)]
struct Observation {
    enabled: bool,
    controls: Vec<serde_json::Value>,
}

fn key() -> Id {
    Id::new("workbench_control_observations")
}

/// One buffer per egui pass; a discarded sizing pass is not published by the
/// browser adapter. Normal workspaces leave observation disabled.
pub(crate) fn begin_observation(context: &Context, enabled: bool) {
    context.data_mut(|data| {
        let observation = data.get_temp_mut_or_default::<Observation>(key());
        observation.enabled = enabled;
        observation.controls.clear();
    });
}

pub(super) fn record(
    ui: &Ui,
    response: &Response,
    label: &str,
    role: &'static str,
    selected: Option<bool>,
) {
    let rect = response.rect.intersect(ui.clip_rect());
    if !rect.is_positive() || !ui.is_rect_visible(rect) {
        return;
    }
    ui.ctx().data_mut(|data| {
        let observation = data.get_temp_mut_or_default::<Observation>(key());
        if observation.enabled {
            observation.controls.push(serde_json::json!({
                "id": response.id.value().to_string(),
                "label": label,
                "role": role,
                "selected": selected,
                "enabled": response.enabled(),
                "rect": [rect.left(), rect.top(), rect.right(), rect.bottom()],
            }));
        }
    });
}

pub(crate) fn observed(context: &Context) -> Option<String> {
    context.data(|data| {
        let observation = data.get_temp::<Observation>(key())?;
        observation
            .enabled
            .then(|| serde_json::Value::Array(observation.controls).to_string())
    })
}
