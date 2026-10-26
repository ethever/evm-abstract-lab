use super::{Entry, Rows};
use crate::app::model::{
    ReportViewId,
    navigation::Focus,
    presentation::{Presentation, ReportIndex},
};
use evm_abstract_protocol::AnalysisNamespace;

#[test]
fn semantic_alias_groups_keep_physical_sources_and_exact_contexts() {
    let mut report = crate::tests::report();
    let mut alias = (*report.model.programs[0]).clone();
    alias.id = 7;
    alias.code_address = Some("0x2222222222222222222222222222222222222222".into());
    report.model.programs.push(std::sync::Arc::new(alias));
    let mut child = report.cfg[0].clone();
    child.id = 91;
    child.program = Some(7);
    child.storage_address = Some("0x3333333333333333333333333333333333333333".into());
    report.cfg.push(child);
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = Focus::source(view, presentation.source(7).unwrap());
    let mut rows = Rows::default();
    rows.prepare(presentation, &focus, None, false);
    rows.filter("");
    assert_eq!(rows.sources.len(), 2);
    assert_eq!(rows.sources[0].group, rows.sources[1].group);
    assert_eq!(rows.sources[0].group_size, 2);
    assert_eq!(rows.contexts.len(), 1);
    assert_eq!(
        rows.contexts[0].focus.graph_selection().unwrap().state,
        Some(91)
    );
    assert!(
        rows.contexts[0]
            .search
            .contains("0x3333333333333333333333333333333333333333")
    );
    assert!(rows.contexts[0].title.contains("mode not recorded"));
    assert_eq!(
        rows.entries
            .iter()
            .filter(|row| matches!(row, Entry::Group(_)))
            .count(),
        1
    );
    rows.filter("0x222222");
    assert_eq!(
        rows.entries
            .iter()
            .filter(|row| matches!(row, Entry::Source(_)))
            .count(),
        1
    );
    let selected = &rows.contexts[0].focus;
    assert!(presentation.validate(selected).is_ok());
    assert_eq!(
        selected.source.as_ref().and_then(|source| source.program()),
        Some(7)
    );
}

#[test]
fn namespace_context_picker_keeps_global_focus_until_an_exact_native_choice() {
    let mut report = crate::tests::report();
    // Repeated IDs deliberately exercise namespace qualification; this clone
    // supplies display records, not a claim of equivalent solver semantics.
    report.model.global = report.model.native.clone();
    let view = ReportViewId::fixture(1, AnalysisNamespace::Baseline);
    let mut index = ReportIndex::default();
    index.update(&report.model, view, None, None);
    let presentation = Presentation::new(&report.model, &index, view, None);
    let focus = Focus::source(view, presentation.source(0).unwrap());
    let original = focus.clone();
    let mut directory = super::Directory::default();
    directory.choose_context(AnalysisNamespace::Native);
    let context = egui::Context::default();
    crate::notation::initialize_fonts(&context);
    let mut output = context.run_ui(egui::RawInput::default(), |ui| {
        assert!(
            directory
                .show_scoped(ui, presentation, &focus, None)
                .is_none()
        );
    });
    output.textures_delta.clear();
    assert_eq!(focus, original);
    assert_eq!(focus.context, None);
    assert_eq!(focus.view.namespace(), AnalysisNamespace::Baseline);
    assert_eq!(directory.scoped.contexts.len(), report.cfg.len());
    for row in &directory.scoped.contexts {
        assert_eq!(row.focus.view.namespace(), AnalysisNamespace::Native);
        assert!(row.focus.graph_selection().unwrap().state.is_some());
        assert!(presentation.validate(&row.focus).is_ok());
    }
    directory.choose_context(AnalysisNamespace::Baseline);
    directory
        .scoped
        .prepare(presentation, &focus, directory.context_namespace, false);
    assert!(
        directory
            .scoped
            .contexts
            .iter()
            .all(|row| row.focus.view.namespace() == AnalysisNamespace::Baseline)
    );
}

#[test]
fn all_contexts_reaches_a_real_programless_callee_without_choosing_it() {
    use evm_abstract_protocol as api;
    let report = evm_abstract_server::analyze::analyze(api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "5f5f5f5f5f60015af100".into(),
            address: Some("0x0000000000000000000000000000000000000101".into()),
        }),
        ..Default::default()
    })
    .unwrap();
    report.validate().unwrap();
    let native = report
        .native
        .as_ref()
        .expect("the finite precompile call retains Native detail");
    let callee = native
        .cfg
        .iter()
        .find(|state| state.program.is_none())
        .expect("the actual precompile has no decoded EVM program");
    let view = ReportViewId::fixture(1, AnalysisNamespace::Native);
    let mut index = ReportIndex::default();
    index.update(&report, view, None, None);
    let presentation = Presentation::new(&report, &index, view, None);
    let focus = presentation.entry_focus(view).unwrap();
    let original = focus.clone();
    let mut rows = Rows::default();
    rows.prepare(presentation, &focus, None, false);
    assert!(
        rows.contexts
            .iter()
            .all(|row| row.focus.graph_selection().unwrap().state != Some(callee.id))
    );
    rows.prepare(presentation, &focus, None, true);
    let choice = rows
        .contexts
        .iter()
        .find(|row| row.focus.graph_selection().unwrap().state == Some(callee.id))
        .expect("All contexts includes the exact retained programless context");
    assert!(presentation.validate(&choice.focus).is_ok());
    assert_ne!(choice.focus.context, original.context);
    assert_eq!(
        focus, original,
        "listing candidates cannot change the active location"
    );
}
