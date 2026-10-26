use super::Calls;
use crate::app::model::{
    ReportViewId,
    navigation::ContextFocus,
    presentation::{ContextTarget, Presentation, ReportIndex},
};
use evm_abstract_protocol::{AnalysisNamespace, CfgEdge, EdgeKind};

#[test]
fn calls_navigate_to_exact_child_source_owner_and_state_and_reject_stale_locations() {
    let mut report = crate::tests::report();
    let mut child = report.cfg[0].clone();
    child.id = 91;
    child.program = Some(7);
    child.code_address = Some("0x2222222222222222222222222222222222222222".into());
    child.storage_address = Some("0x3333333333333333333333333333333333333333".into());
    let source = crate::tests::source_program(
        7,
        child.code_address.clone(),
        report.model.programs[0].blocks.clone(),
    );
    report.model.programs.push(source);
    report.cfg.push(child);
    report.edges.push(CfgEdge {
        id: 900,
        from: 0,
        to: 91,
        kind: EdgeKind::Call,
    });
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = presentation
        .focus_context(ContextTarget {
            view,
            context: ContextFocus::Graph { state: 0 },
        })
        .unwrap();
    let mut calls = Calls::default();
    calls.prepare(presentation, &focus);
    assert_eq!(calls.links.len(), 1);
    let target = &calls.links[0];
    assert_eq!(
        target.focus.context,
        Some(ContextFocus::Graph { state: 91 })
    );
    assert_eq!(
        target
            .focus
            .source
            .as_ref()
            .and_then(|source| source.program()),
        Some(7)
    );
    assert!(
        target
            .detail
            .contains("0x3333333333333333333333333333333333333333")
    );
    assert!(presentation.validate(&target.focus).is_ok());
    let mut stale = focus;
    stale.view = ReportViewId::fixture(2, AnalysisNamespace::Native);
    calls.prepare(presentation, &stale);
    assert!(calls.links.is_empty());
}
