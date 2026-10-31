use crate::{
    controls,
    decompile::{
        inspector,
        model::{Publication, ViewState},
        rows,
        tests::fixture,
    },
};
use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    JobId,
    decompile_model::{TransformEvent, records},
};

#[test]
fn opened_large_event_section_prepares_only_visible_rows_and_reuses_new_entity_index() {
    let mut artifact = fixture::large(1500);
    let stage = &artifact.stages[0];
    let created: Vec<_> = records(&stage.ir).keys().copied().collect();
    assert!(created.len() > 4000);
    artifact.events = vec![TransformEvent {
        id: 7,
        from_stage: None,
        to_stage: stage.id,
        rule: "source construction".into(),
        reason: "All original decode entities".into(),
        inputs: vec![],
        outputs: vec![],
        created,
        deleted: vec![],
        clones: vec![],
        redirects: vec![],
        validation: stage.validation.clone(),
    }];
    let mut state = ViewState::default();
    state.install(
        &artifact,
        Publication {
            job: JobId(1),
            version: fixture::version(1),
        },
    );
    state.selected = None;
    state.source_pc = None;
    state.event = Some(7);
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut frame = |events| {
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 900.0))),
                events,
                ..RawInput::default()
            },
            |ui| {
                controls::begin_observation(ui.ctx(), true);
                rows::begin_frame(ui.ctx());
                inspector::show(ui, &artifact, &mut state);
            },
        );
        output.textures_delta.clear();
        serde_json::from_str::<Vec<serde_json::Value>>(&controls::observed(&ctx).unwrap()).unwrap()
    };
    let controls = frame(vec![]);
    let section = controls
        .iter()
        .find(|control| {
            control["role"] == "section" && control["label"] == "New and removed entities"
        })
        .unwrap();
    let rect = &section["rect"];
    let at = Pos2::new(
        ((rect[0].as_f64().unwrap() + rect[2].as_f64().unwrap()) / 2.0) as f32,
        ((rect[1].as_f64().unwrap() + rect[3].as_f64().unwrap()) / 2.0) as f32,
    );
    frame(vec![
        Event::PointerMoved(at),
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
        Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]);
    let visible = frame(vec![]);
    let new = visible
        .iter()
        .filter(|control| {
            control["role"] == "decompile-entity-link"
                && control["label"].as_str().unwrap().starts_with("New ")
        })
        .count();
    assert!(new > 0 && new < 12, "{visible:?}");
    assert!(rows::prepared(&ctx) < 20);
    let first = state.new_entities(&artifact.events[0]);
    let second = state.new_entities(&artifact.events[0]);
    assert!(std::rc::Rc::ptr_eq(&first, &second));
}
