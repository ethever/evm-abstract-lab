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
const RESIDUAL_CYCLE: &str = "5f3561001b575b5f54600101805f556004901061001b57610030565b5f54600101805f556004901061000657610030565b00";

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

#[test]
fn readability_resubmission_preserves_the_artifacts_declared_execution_scope() {
    let artifact = analyze(DecompileRequest {
        bytecode: BRANCH.into(),
        execution_domain:
            evm_abstract_protocol::decompile_model::ExecutionDomain::EthereumTransactions,
        ..Default::default()
    });
    let editor = Editor::new(artifact);
    let request = editor.preferences.request(&editor.artifact);
    assert_eq!(
        request.execution_domain,
        editor.artifact.config.execution_domain
    );
    assert_eq!(
        request.execution_domain,
        evm_abstract_protocol::decompile_model::ExecutionDomain::EthereumTransactions
    );
}

#[test]
fn legacy_seven_region_decisions_remain_readable_and_are_not_retargeted_on_resubmission() {
    let artifact = analyze(DecompileRequest {
        bytecode: BRANCH.into(),
        ..Default::default()
    });
    let mut original = Editor::new(artifact);
    original.click("Compact");
    original.click("Dispatcher");
    let actual = analyze(original.click("Apply reading choices").unwrap());
    assert!(!actual.readability.is_empty() && !actual.config.readability.regions.is_empty());
    let mut legacy = actual;
    // The source and its empty-REVERT guards predate occurrence-local data.
    // Drop only that optional modern metadata to represent a schema-7 producer.
    for stage in &mut legacy.stages {
        if let Some(program) = &mut stage.ir.lifted {
            assert!(
                program
                    .controls
                    .iter()
                    .filter_map(|control| control.guard.as_ref())
                    .all(|guard| !matches!(
                        guard.payload,
                        evm_abstract_protocol::decompile_model::LiftedGuardPayload::Data(_)
                    ))
            );
            program.revert_data.clear();
        }
    }
    legacy.schema_version = 7;
    let source = legacy.readability_source();
    assert_ne!(source, readability_source(&legacy.input, &legacy.config));
    for decision in &mut legacy.readability {
        decision.region.source = source.clone();
    }
    for preference in &mut legacy.config.readability.regions {
        preference.region.source = source.clone();
    }
    assert!(legacy.admits_schema());
    crate::decompile::model::check_artifact(&legacy).unwrap();
    let retained = legacy.readability.clone();
    let editor = Editor::new(legacy.clone());
    assert_eq!(editor.preferences.source.as_ref(), Some(&source));
    assert_eq!(
        editor.artifact.readability, retained,
        "legacy region identities remain unchanged for display"
    );
    assert!(
        editor
            .preferences
            .notice
            .unwrap()
            .contains("cleared when reanalyzing")
    );
    let request = editor.preferences.request(&legacy);
    assert!(
        request.readability.regions.is_empty(),
        "old region IDs cannot be sent as current source choices"
    );
    assert_eq!(
        request.readability.profile,
        legacy.config.readability.profile
    );
    assert_eq!(
        request.readability.procedures,
        legacy.config.readability.procedures
    );
    let refreshed = analyze(request);
    assert!(
        refreshed
            .readability
            .iter()
            .all(|decision| decision.region.source != source)
    );
}

#[test]
fn residual_dispatcher_reports_the_backend_trigger_in_the_rendered_choice() {
    let artifact = analyze(DecompileRequest {
        bytecode: RESIDUAL_CYCLE.into(),
        ..Default::default()
    });
    assert!(
        artifact
            .readability
            .iter()
            .all(|decision| decision.reason != ReadabilityReason::MultiEntryControl)
    );
    let region = artifact
        .readability
        .iter()
        .find(|decision| {
            decision.selected == StructureForm::Dispatcher
                && decision.reason == ReadabilityReason::ResidualCycle
        })
        .expect("a source residual cycle retained an actual dispatcher")
        .region
        .clone();
    let mut editor = Editor::new(artifact);
    editor.preferences.region = Some(region);
    editor.frame(vec![]);
    assert!(editor.controls.iter().any(|control| {
        control["role"] == "decompile-reading-choice"
            && control["label"]
                == "Selected: Dispatcher. A cycle remained outside admitted natural loops."
    }));
}

#[test]
fn rendered_fallbacks_keep_the_cause_when_a_natural_preference_is_unavailable() {
    let ctx = Context::default();
    for (reason, detail) in [
        (
            ReadabilityReason::ResidualCycle,
            "A cycle remained outside admitted natural loops",
        ),
        (
            ReadabilityReason::ControlReentry,
            "Structuring revisited a block on the active path",
        ),
        (
            ReadabilityReason::Expansion,
            "Retained compact control after source expansion",
        ),
        (
            ReadabilityReason::PlanningFallback,
            "Planning admission retained compact control",
        ),
        (
            ReadabilityReason::DepthFallback,
            "Structure depth required a compact representation",
        ),
        (
            ReadabilityReason::ValidationFallback,
            "Proposed structure failed validation; used a verified dispatcher",
        ),
        (
            ReadabilityReason::MultiEntryControl,
            "Legacy dispatcher fallback; detailed cause unavailable",
        ),
    ] {
        let decision = RegionDecision {
            region: RegionAnchor {
                source: ArtifactId("source".into()),
                entry: EntityId(1),
                continuation: None,
                loop_header: None,
            },
            preference: StructurePreference::Natural,
            selected: StructureForm::Dispatcher,
            reason,
            natural: None,
            dispatcher: None,
        };
        let mut output = ctx.run_ui(RawInput::default(), |ui| {
            controls::begin_observation(ui.ctx(), true);
            decision_details(ui, &decision);
        });
        output.textures_delta.clear();
        let controls: Vec<serde_json::Value> =
            serde_json::from_str(&controls::observed(&ctx).unwrap()).unwrap();
        let rendered = controls
            .iter()
            .find(|control| control["role"] == "decompile-reading-choice")
            .unwrap()["label"]
            .as_str()
            .unwrap();
        assert_eq!(
            rendered,
            format!("Selected: Dispatcher. {detail}. Requested Natural form unavailable.")
        );
        assert!(!rendered.contains("Multiple-entry") && !rendered.contains("irreducible"));
    }
}
