use super::*;
use crate::decompile::model::Publication;
use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{DecimalU64, JobId, JobState, PublicationVersion};
use evm_abstract_server::jobs::{Config, Pool};
use std::{
    thread,
    time::{Duration, Instant},
};

const BRANCH: &str =
    "5f356100115760403561001d57610021565b60203561001d57610021565b5f5ffd5b60775f5500";

fn analyze(request: DecompileRequest) -> Artifact {
    let pool = Pool::new(Config {
        workers: 1,
        queue_capacity: 1,
        retained_jobs: 1,
    })
    .unwrap();
    let job = pool.submit_decompile(request).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while pool.decompile_status(job.id).unwrap().state != JobState::Completed {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    let artifact = pool
        .decompile_result(job.id)
        .unwrap()
        .artifact
        .as_ref()
        .clone();
    pool.shutdown().unwrap();
    artifact
}

struct Editor {
    ctx: Context,
    preferences: Preferences,
    artifact: Artifact,
    state: ViewState,
    controls: Vec<serde_json::Value>,
}

impl Editor {
    fn new(artifact: Artifact) -> Self {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let mut result = Self {
            ctx,
            preferences: Preferences::default(),
            artifact: artifact.clone(),
            state: ViewState::default(),
            controls: Vec::new(),
        };
        result.install(artifact);
        result.frame(vec![]);
        result.frame(vec![]);
        result.click("Reading choices");
        result.frame(vec![]);
        result
    }
    fn install(&mut self, artifact: Artifact) {
        self.state.install(
            &artifact,
            Publication {
                job: JobId(10),
                version: PublicationVersion(DecimalU64(1)),
            },
        );
        self.preferences.install(&artifact, &self.state);
        self.artifact = artifact;
    }
    fn frame(&mut self, events: Vec<Event>) -> Option<DecompileRequest> {
        let mut request = None;
        let mut output = self.ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 1600.0))),
                events,
                ..RawInput::default()
            },
            |ui| {
                controls::begin_observation(ui.ctx(), true);
                request = self
                    .preferences
                    .show(ui, &self.artifact, &self.state, false);
            },
        );
        output.textures_delta.clear();
        self.controls = serde_json::from_str(&controls::observed(&self.ctx).unwrap()).unwrap();
        request
    }
    fn click(&mut self, label: &str) -> Option<DecompileRequest> {
        self.frame(vec![]);
        let control = self
            .controls
            .iter()
            .find(|control| control["label"] == label)
            .unwrap_or_else(|| panic!("missing {label}: {:?}", self.controls));
        let rect: [f32; 4] = serde_json::from_value(control["rect"].clone()).unwrap();
        let pos = Pos2::new((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
        for pressed in [true, false] {
            let request = self.frame(vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]);
            if request.is_some() {
                return request;
            }
        }
        None
    }
}

#[test]
fn rendered_region_choices_submit_original_input_and_change_verified_backend_form() {
    let artifact = analyze(DecompileRequest {
        bytecode: BRANCH.into(),
        ..Default::default()
    });
    assert!(!artifact.readability.is_empty());
    let mut editor = Editor::new(artifact.clone());
    let anchor = editor
        .preferences
        .region
        .clone()
        .expect("entry selection finds actual backend region");
    assert!(editor.click("Compact").is_none());
    assert!(editor.click("Inline").is_none());
    assert!(editor.click("Dispatcher").is_none());
    let request = editor
        .click("Apply reading choices")
        .expect("actual apply action submits");
    assert_eq!(request.bytecode, BRANCH);
    assert_eq!(request.limits, artifact.config.limits);
    assert_eq!(request.readability.profile, ReadabilityProfile::Compact);
    assert_eq!(request.readability.procedures, ProcedurePreference::Inline);
    assert_eq!(
        request.readability.regions,
        [RegionPreference {
            region: anchor.clone(),
            structure: StructurePreference::Dispatcher
        }]
    );
    let dispatched = analyze(request);
    let chosen = dispatched
        .readability
        .iter()
        .find(|decision| decision.region == anchor)
        .unwrap();
    assert_eq!(chosen.selected, StructureForm::Dispatcher);
    assert_eq!(chosen.reason, ReadabilityReason::UserPreference);
    editor.install(dispatched);
    assert_eq!(editor.preferences.region, Some(anchor.clone()));
    editor.frame(vec![]);
    assert!(editor.controls.iter().any(|control| {
        control["role"] == "decompile-reading-choice"
            && control["label"]
                .as_str()
                .unwrap()
                .contains("Selected: Dispatcher")
    }));
    assert!(editor.click("Natural").is_none());
    let natural = analyze(editor.click("Apply reading choices").unwrap());
    assert_eq!(
        natural
            .readability
            .iter()
            .find(|decision| decision.region == anchor)
            .unwrap()
            .selected,
        StructureForm::Natural
    );
}

#[test]
fn changed_source_rejects_and_clears_old_region_choices_despite_reused_entry_id() {
    let artifact = analyze(DecompileRequest {
        bytecode: BRANCH.into(),
        ..Default::default()
    });
    let mut editor = Editor::new(artifact);
    editor.click("Dispatcher");
    let mut request = editor.click("Apply reading choices").unwrap();
    let old_anchor = request.readability.regions[0].region.clone();
    request.bytecode = "00".into();
    let changed = analyze(request);
    assert!(changed.diagnostics.iter().any(|diagnostic| diagnostic.code
        == evm_abstract_protocol::decompile_model::DiagnosticCode::Unsupported));
    assert!(
        changed
            .readability
            .iter()
            .all(|decision| decision.region != old_anchor)
    );
    editor.install(changed);
    assert!(editor.preferences.config.regions.is_empty());
    assert!(editor.preferences.region.is_none());
    assert!(editor.preferences.notice.is_some());
    assert!(
        editor
            .preferences
            .request(&editor.artifact)
            .readability
            .regions
            .is_empty()
    );
}

#[test]
fn foreign_or_dangling_decision_records_cannot_supply_region_controls() {
    let artifact = analyze(DecompileRequest {
        bytecode: BRANCH.into(),
        ..Default::default()
    });
    super::super::model::check_artifact(&artifact).unwrap();
    let mut foreign = artifact.clone();
    foreign.readability[0].region.source = ArtifactId("different input".into());
    assert!(
        super::super::model::check_artifact(&foreign)
            .unwrap_err()
            .contains("recovered source region")
    );
    let mut dangling = artifact;
    dangling.readability[0].region.continuation = Some(EntityId(u64::MAX));
    assert!(
        super::super::model::check_artifact(&dangling)
            .unwrap_err()
            .contains("recovered source region")
    );
}
