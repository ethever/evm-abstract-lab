use super::*;
use crate::app::model::presentation::{ContextTarget, ReportIndex};
use evm_abstract_protocol::{
    AnalysisNamespace, AnalysisReport, DecimalU64, JobId, JobPublication, PublicationVersion,
};

#[test]
fn back_forward_restore_each_exact_location_and_camera_without_selection_noise() {
    let report: AnalysisReport = crate::tests::report().into();
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report, view, None, None);
    let presentation = Presentation::new(&report, &index, view, None);
    let first = presentation.entry_focus(view).unwrap();
    let second = presentation
        .focus_context(ContextTarget {
            view,
            context: ContextFocus::Graph { state: 2 },
        })
        .unwrap();
    let mut navigator = Navigator::default();
    navigator
        .apply(&presentation, Intent::Open(first.clone()))
        .unwrap();
    let camera = CameraBookmark::new(0.75, [19.0, -37.0]).unwrap();
    navigator.remember_camera(camera);
    navigator.remember_route(crate::app::View::Disassembly);
    navigator
        .apply(&presentation, Intent::Open(second.clone()))
        .unwrap();
    assert!(navigator.can_back());
    assert!(!navigator.can_forward());
    navigator.remember_route(crate::app::View::Ssa);
    navigator.apply(&presentation, Intent::Back).unwrap();
    assert_eq!(
        navigator.current().unwrap(),
        &Location {
            focus: first.clone(),
            camera: Some(camera),
            route: crate::app::View::Disassembly,
        }
    );
    navigator.apply(&presentation, Intent::Forward).unwrap();
    assert_eq!(navigator.current().unwrap().focus, second);
    assert_eq!(navigator.current().unwrap().route, crate::app::View::Ssa);
    navigator.apply(&presentation, Intent::Back).unwrap();
    let mut instruction = first;
    instruction.evidence = EvidenceFocus::Graph(crate::app::Selection {
        state: Some(0),
        pc: Some(0),
        value: None,
    });
    navigator
        .synchronize(&presentation, instruction.clone())
        .unwrap();
    assert_eq!(navigator.current().unwrap().camera, Some(camera));
    assert!(navigator.can_forward());
    navigator
        .apply(&presentation, Intent::Open(instruction))
        .unwrap();
    assert!(
        navigator.can_forward(),
        "opening the current location is idempotent"
    );
}

#[test]
fn failed_navigation_is_atomic_and_a_new_branch_retires_forward_history() {
    let report: AnalysisReport = crate::tests::report().into();
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report, view, None, None);
    let presentation = Presentation::new(&report, &index, view, None);
    let first = presentation.entry_focus(view).unwrap();
    let mut navigator = Navigator::default();
    navigator
        .apply(&presentation, Intent::Open(first.clone()))
        .unwrap();
    let before = navigator.current().cloned();
    let mut stale = first.clone();
    stale.view = ReportViewId::fixture(2, AnalysisNamespace::Native);
    assert_eq!(
        navigator.apply(&presentation, Intent::Open(stale)),
        Err(NavigationError::StaleReport)
    );
    assert_eq!(navigator.current(), before.as_ref());
    assert!(!navigator.can_back());
    for state in [1, 2] {
        let next = presentation
            .focus_context(ContextTarget {
                view,
                context: ContextFocus::Graph { state },
            })
            .unwrap();
        navigator.apply(&presentation, Intent::Open(next)).unwrap();
    }
    navigator.apply(&presentation, Intent::Back).unwrap();
    assert!(navigator.can_forward());
    navigator.apply(&presentation, Intent::Open(first)).unwrap();
    assert!(!navigator.can_forward());
}

#[test]
fn richer_publications_preserve_valid_history_but_new_lineages_clear_it() {
    let report: AnalysisReport = crate::tests::report().into();
    let version = |value| PublicationVersion(DecimalU64(value));
    let mut publication = JobPublication {
        version: version(1),
        minimum_version: None,
        global_version: None,
        native_version: Some(version(1)),
        model_closed: true,
    };
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native)
        .for_namespace(AnalysisNamespace::Native, Some(&publication));
    let mut index = ReportIndex::default();
    index.update(&report, view, Some(&publication), Some(JobId(1)));
    let presentation = Presentation::new(&report, &index, view, Some(&publication));
    let first = presentation.entry_focus(view).unwrap();
    let second = presentation
        .focus_context(ContextTarget {
            view,
            context: ContextFocus::Graph { state: 2 },
        })
        .unwrap();
    let mut navigator = Navigator::default();
    navigator.apply(&presentation, Intent::Open(first)).unwrap();
    navigator
        .apply(&presentation, Intent::Open(second))
        .unwrap();
    navigator.remember_camera(CameraBookmark::new(1.25, [2.0, 3.0]).unwrap());
    let current = navigator.current().cloned();
    publication.version = version(2);
    index.update(&report, view, Some(&publication), Some(JobId(1)));
    let presentation = Presentation::new(&report, &index, view, Some(&publication));
    navigator.reconcile(&presentation, None);
    assert_eq!(navigator.current(), current.as_ref());
    assert!(navigator.can_back());
    let replacement = ReportViewId::fixture(2, AnalysisNamespace::Native)
        .for_namespace(AnalysisNamespace::Native, Some(&publication));
    index.update(&report, replacement, Some(&publication), Some(JobId(1)));
    let presentation = Presentation::new(&report, &index, replacement, Some(&publication));
    navigator.reconcile(
        &presentation,
        Some(presentation.entry_focus(replacement).unwrap()),
    );
    assert_eq!(navigator.current().unwrap().focus.view, replacement);
    assert_eq!(navigator.current().unwrap().camera, None);
    assert!(!navigator.can_back());
    assert!(!navigator.can_forward());
}

#[test]
fn malformed_camera_never_replaces_a_valid_bookmark() {
    assert!(CameraBookmark::new(f32::NAN, [0.0; 2]).is_none());
    assert!(CameraBookmark::new(1.0, [f32::INFINITY, 0.0]).is_none());
    assert!(CameraBookmark::new(0.0, [0.0; 2]).is_none());
    let report: AnalysisReport = crate::tests::report().into();
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report, view, None, None);
    let presentation = Presentation::new(&report, &index, view, None);
    let mut navigator = Navigator::default();
    navigator
        .apply(
            &presentation,
            Intent::Open(presentation.entry_focus(view).unwrap()),
        )
        .unwrap();
    let valid = CameraBookmark::new(1.0, [5.0, 6.0]).unwrap();
    navigator.remember_camera(valid);
    navigator.remember_camera(CameraBookmark {
        zoom: -1.0,
        pan: [0.0; 2],
    });
    assert_eq!(navigator.current().unwrap().camera, Some(valid));
}
