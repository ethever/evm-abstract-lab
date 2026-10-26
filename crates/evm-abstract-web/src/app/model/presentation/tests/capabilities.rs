//! Producer receipts remain intact while consumer profiles exercise currency,
//! missing optional detail, and catalog append independently of availability.

use super::*;

const CODE: &str = "60016005565b60020100";

fn request() -> api::AnalyzeRequest {
    api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: CODE.into(),
            address: None,
        }),
        ..Default::default()
    }
}

fn actual_native(partial: bool) -> AnalysisReport {
    let mut request = request();
    if partial {
        // Deliberately request a partial native receipt, not a performance case
        // or a substitute for the unchanged default Hybrid acceptance inputs.
        request.limits.max_transfers = 1;
    }
    let report =
        evm_abstract_server::analyze::captured::native_bytecode(&request, &Default::default())
            .unwrap();
    report.validate().unwrap();
    assert_eq!(report.native.as_ref().unwrap().ssa.complete, !partial);
    report
}

fn receipt(report: &AnalysisReport) -> Selection {
    let graph = report.native.as_ref().unwrap();
    graph
        .ssa
        .blocks
        .iter()
        .find_map(|block| {
            block.instructions.iter().find_map(|instruction| {
                Some(Selection {
                    state: Some(block.state),
                    pc: Some(instruction.pc),
                    value: Some(*instruction.results.first()?),
                })
            })
        })
        .expect("actual producer instruction and value")
}

fn with_older_input() -> AnalysisReport {
    let mut report = evm_abstract_server::analyze::analyze(request()).unwrap();
    report.validate().unwrap();
    // A validated consumer qualification profile: preserve the actual Native
    // graph, snapshot, binding and numerical receipts, and advance only the
    // RPC revision chain. This does not claim a new RPC execution occurred.
    let hybrid = report.hybrid.as_mut().unwrap();
    let precision = hybrid.native_coverage().clone();
    let detail_revision = precision.detail_revision.unwrap();
    let current_revision = api::DecimalU64(detail_revision.0 + 1);
    let acquisition = api::AcquisitionReport {
        rounds: 1,
        fetched_accounts: vec![],
        failed_accounts: vec![],
        fetched_storage: vec![],
        failed_storage: vec![],
        failures: vec![],
        requests: 0,
        states_created: report.native.as_ref().unwrap().states.len(),
    };
    hybrid.rpc = Some(api::RpcHybridCoverage {
        snapshot: precision.snapshot.clone(),
        baseline_revision: hybrid.coverage.baseline_revision,
        current_revision,
        detail_revision: Some(detail_revision),
        detail_at_current_revision: false,
        baseline_relation: api::RpcBaselineRelation::InitialFactRestriction,
        restrictions: vec![api::RpcFactRestriction {
            previous: hybrid.coverage.baseline_revision,
            current: current_revision,
            added_observations: 1,
        }],
        precision: api::RpcPrecisionOutcome::Incomplete,
        acquisition: acquisition.clone(),
        work: report.work,
        precision_work: precision.work.preparation + precision.work.detail,
        max_precision_work: api::DecimalU64(precision.work.detail_allowance),
        transfers: report.transfers,
        failures: vec![],
        preparation_failure: None,
    });
    hybrid.precision = Some(precision);
    hybrid.coverage.chosen = api::HybridInterpretation::Process;
    report.acquisition = Some(acquisition);
    report.validate().unwrap();
    report
}

#[test]
fn older_condition_native_receipts_stay_available_with_their_actual_revision_and_binding() {
    let report = with_older_input();
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native);
    let index = index(&report, view);
    let presentation = Presentation::new(&report, &index, view, None);
    let selected = receipt(&report);
    let focus = presentation.graph_focus(view, selected, None).unwrap();
    assert_eq!(
        presentation.capabilities(&focus).unwrap().ssa,
        Capability::Available
    );
    assert_eq!(focus.graph_selection(), Some(selected));
    let Some(ResolvedContext::Graph {
        graph,
        coverage,
        at_current_revision,
        binding,
        ..
    }) = presentation.resolve_context(&focus).unwrap()
    else {
        panic!("Native evidence")
    };
    let hybrid = report.hybrid.as_ref().unwrap();
    let rpc = hybrid.rpc.as_ref().unwrap();
    assert_eq!(Some(graph.metadata.revision), rpc.detail_revision);
    assert_ne!(graph.metadata.revision, rpc.current_revision);
    assert!(!at_current_revision);
    assert_eq!(
        coverage,
        Some(api::BlockCoverage::Current),
        "local input coverage does not imply latest RPC input"
    );
    assert_eq!(binding, Some(&hybrid.native_coverage().detail_binding));
    assert!(matches!(
        binding,
        Some(api::HybridNativeBinding::SymbolicOwnerNamespace(_))
    ));
    assert!(graph.ssa.complete);
    assert!(
        presentation
            .graph_focus(
                view,
                Selection {
                    pc: Some(usize::MAX),
                    ..selected
                },
                None
            )
            .is_err()
    );
    assert!(
        presentation
            .graph_focus(
                view,
                Selection {
                    value: Some(usize::MAX),
                    ..selected
                },
                None
            )
            .is_err()
    );
}

#[test]
fn incomplete_native_closure_does_not_hide_retained_receipts() {
    let mut report = with_older_input();
    let precision = report.hybrid.as_mut().unwrap().precision.as_mut().unwrap();
    precision.detail_current = false;
    precision.detail = api::HybridDetailState::Incomplete;
    report.validate().unwrap();
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native);
    let index = index(&report, view);
    let presentation = Presentation::new(&report, &index, view, None);
    let focus = presentation
        .graph_focus(view, receipt(&report), None)
        .unwrap();
    assert!(
        !report
            .hybrid
            .as_ref()
            .unwrap()
            .native_coverage()
            .detail_current
    );
    assert!(report.native.as_ref().unwrap().ssa.complete);
    assert_eq!(
        presentation.capabilities(&focus).unwrap().ssa,
        Capability::Available
    );
    assert!(matches!(
        presentation.resolve_context(&focus).unwrap(),
        Some(ResolvedContext::Graph {
            at_current_revision: false,
            coverage: Some(api::BlockCoverage::Current),
            ..
        })
    ));
}

#[test]
fn actual_partial_and_retained_stale_ssa_are_browseable_without_becoming_current() {
    for stale in [false, true] {
        let mut report = actual_native(true);
        let selected = receipt(&report);
        if stale {
            // Only the retained block's currency changes. Its original
            // instruction/value payload is neither reconstructed nor joined.
            let graph = Arc::make_mut(report.native.as_mut().unwrap());
            graph
                .ssa
                .blocks
                .iter_mut()
                .find(|block| Some(block.state) == selected.state)
                .unwrap()
                .coverage = api::BlockCoverage::Stale;
        }
        report.validate().unwrap();
        let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native);
        let index = index(&report, view);
        let presentation = Presentation::new(&report, &index, view, None);
        let focus = presentation.graph_focus(view, selected, None).unwrap();
        assert_eq!(
            presentation.capabilities(&focus).unwrap().ssa,
            Capability::Available
        );
        let Some(ResolvedContext::Graph {
            graph, coverage, ..
        }) = presentation.resolve_context(&focus).unwrap()
        else {
            panic!("Native context")
        };
        assert!(!graph.ssa.complete);
        assert_eq!(
            coverage,
            Some(if stale {
                api::BlockCoverage::Stale
            } else {
                api::BlockCoverage::Current
            })
        );
        assert!(
            presentation
                .graph_focus(
                    view,
                    Selection {
                        pc: Some(usize::MAX),
                        ..selected
                    },
                    None
                )
                .is_err()
        );
        assert!(
            presentation
                .graph_focus(
                    view,
                    Selection {
                        value: Some(usize::MAX),
                        ..selected
                    },
                    None
                )
                .is_err()
        );
    }
}

#[test]
fn pending_entry_keeps_ssa_unavailable_and_rejects_executed_pc() {
    let report = actual_native(true);
    let graph = report.native.as_ref().unwrap();
    let pending = graph
        .ssa
        .blocks
        .iter()
        .find(|block| block.coverage == api::BlockCoverage::Unexecuted)
        .expect("actual pending successor");
    assert!(pending.instructions.is_empty());
    let state = graph
        .cfg
        .iter()
        .find(|state| state.id == pending.state)
        .unwrap();
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native);
    let index = index(&report, view);
    let presentation = Presentation::new(&report, &index, view, None);
    let focus = presentation
        .focus_context(ContextTarget {
            view,
            context: ContextFocus::Graph { state: state.id },
        })
        .unwrap();
    assert_eq!(
        presentation.capabilities(&focus).unwrap().ssa,
        Capability::Unavailable(UnavailableReason::Unexecuted)
    );
    assert!(matches!(
        presentation.resolve_context(&focus).unwrap(),
        Some(ResolvedContext::Graph {
            coverage: Some(api::BlockCoverage::Unexecuted),
            ..
        })
    ));
    assert_eq!(
        presentation.graph_focus(
            view,
            Selection {
                state: Some(state.id),
                pc: state.start_pc,
                value: None
            },
            None
        ),
        Err(NavigationError::MissingEvidence)
    );
}

#[test]
fn withheld_ssa_and_foreign_values_remain_unavailable_or_rejected() {
    let mut report = actual_native(false);
    let selected = receipt(&report);
    let graph = report.native.as_ref().unwrap();
    let foreign = graph
        .ssa
        .blocks
        .iter()
        .filter(|block| Some(block.state) != selected.state)
        .flat_map(|block| &block.instructions)
        .flat_map(|instruction| &instruction.results)
        .next()
        .copied()
        .expect("actual result from another state");
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native);
    {
        let index = index(&report, view);
        let presentation = Presentation::new(&report, &index, view, None);
        assert_eq!(
            presentation.graph_focus(
                view,
                Selection {
                    value: Some(foreign),
                    ..selected
                },
                None
            ),
            Err(NavigationError::ContextMismatch)
        );
    }
    let graph = Arc::make_mut(report.native.as_mut().unwrap());
    graph
        .ssa
        .blocks
        .retain(|block| Some(block.state) != selected.state);
    graph.ssa.complete = false;
    report.validate().unwrap();
    let index = index(&report, view);
    let presentation = Presentation::new(&report, &index, view, None);
    let focus = presentation
        .focus_context(ContextTarget {
            view,
            context: ContextFocus::Graph {
                state: selected.state.unwrap(),
            },
        })
        .unwrap();
    assert_eq!(
        presentation.capabilities(&focus).unwrap().ssa,
        Capability::Unavailable(UnavailableReason::NoSsaReceipt)
    );
    assert_eq!(
        presentation.graph_focus(view, selected, None),
        Err(NavigationError::MissingEvidence)
    );
}

#[test]
fn catalog_aliases_require_an_explicit_context_before_advertising_analysis_cfg() {
    let mut report = actual_native(false);
    let version = |value| api::PublicationVersion(api::DecimalU64(value));
    let mut publication = api::JobPublication {
        version: version(1),
        minimum_version: None,
        global_version: None,
        native_version: Some(version(1)),
        model_closed: true,
    };
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native)
        .for_namespace(api::AnalysisNamespace::Native, Some(&publication));
    let program = report
        .native
        .as_ref()
        .unwrap()
        .cfg
        .iter()
        .find_map(|state| state.program)
        .unwrap();
    let mut index = ReportIndex::default();
    index.update(&report, view, Some(&publication), Some(JobId(17)));
    assert!(!index.contexts_for_program(program).is_empty());
    let source = Presentation::new(&report, &index, view, Some(&publication))
        .source(program)
        .unwrap();

    let mut alias = report
        .programs
        .iter()
        .find(|candidate| candidate.id == program)
        .unwrap()
        .as_ref()
        .clone();
    alias.id = report.programs.len();
    alias.code_address = Some(format!("0x{}02", "00".repeat(19)));
    let alias_id = alias.id;
    report.programs.push(Arc::new(alias));
    report.validate().unwrap();
    publication.version = version(2); // unchanged analyzed graph; catalog grows
    index.update(&report, view, Some(&publication), Some(JobId(17)));
    assert!(index.contexts_for_program(alias_id).is_empty());
    let presentation = Presentation::new(&report, &index, view, Some(&publication));
    let explicit = presentation
        .focus_context(index.contexts_for_program(program)[0])
        .unwrap();
    assert_eq!(
        presentation.capabilities(&explicit).unwrap().cfg,
        Capability::Available
    );
    for source in [source, presentation.source(alias_id).unwrap()] {
        let source_only = Focus::source(view, source);
        let capabilities = presentation.capabilities(&source_only).unwrap();
        assert_eq!(capabilities.code, Capability::Available);
        assert_eq!(
            capabilities.cfg,
            Capability::Unavailable(UnavailableReason::SelectContext)
        );
        assert!(
            presentation
                .resolve_context(&source_only)
                .unwrap()
                .is_none()
        );
        assert!(source_only.graph_selection().is_none());
        let mut navigator = Navigator::default();
        navigator
            .apply(&presentation, Intent::Open(source_only.clone()))
            .unwrap();
        assert_eq!(navigator.current().unwrap().focus, source_only);
        assert_eq!(navigator.current().unwrap().route, crate::app::View::Graph);
    }
    let mut wrong_alias = explicit;
    wrong_alias.source = presentation.source(alias_id);
    assert_eq!(
        presentation.validate(&wrong_alias),
        Err(NavigationError::SourceMismatch)
    );
}

#[test]
fn global_source_choices_do_not_implicitly_select_may_or_native_contexts() {
    let report = process();
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Baseline).minimum(None);
    let index = index(report, view);
    let presentation = Presentation::new(report, &index, view, None);
    let actual = presentation.entry_focus(view).unwrap();
    let source_only = Focus::source(view, actual.source.clone().unwrap());
    assert!(
        !index
            .contexts_for_program(source_only.source.as_ref().unwrap().program().unwrap())
            .is_empty()
    );
    let capabilities = presentation.capabilities(&source_only).unwrap();
    assert_eq!(capabilities.code, Capability::Available);
    assert_eq!(
        capabilities.cfg,
        Capability::Unavailable(UnavailableReason::SelectContext)
    );
    assert_eq!(
        presentation.capabilities(&actual).unwrap().cfg,
        Capability::Available
    );
    assert_eq!(
        presentation.capabilities(&actual).unwrap().ssa,
        Capability::Unavailable(UnavailableReason::ConservativeEvidence)
    );
    let mut invented = actual;
    invented.evidence = EvidenceFocus::Graph(Selection {
        state: Some(0),
        pc: Some(0),
        value: Some(0),
    });
    assert_eq!(
        presentation.validate(&invented),
        Err(NavigationError::ContextMismatch)
    );
    assert!(source_only.minimum_selection().is_none());
    assert!(source_only.graph_selection().is_none());
}
