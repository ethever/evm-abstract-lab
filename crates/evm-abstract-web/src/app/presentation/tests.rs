use super::*;
use crate::app::{Selection, model::navigation::*};
use evm_abstract_protocol as api;
use std::sync::{Arc, OnceLock};

fn native() -> AnalysisReport {
    crate::tests::report().into()
}

fn process() -> &'static AnalysisReport {
    static REPORT: OnceLock<AnalysisReport> = OnceLock::new();
    REPORT.get_or_init(|| {
        let report = evm_abstract_server::analyze::analyze(api::AnalyzeRequest {
            input: api::AnalysisInput::Bytecode(api::BytecodeInput {
                bytecode: "5f5f5f5f5f5f355af100".into(),
                address: None,
            }),
            ..Default::default()
        })
        .unwrap();
        report.validate().unwrap();
        report
    })
}

fn index(report: &AnalysisReport, view: ReportViewId) -> ReportIndex {
    let mut index = ReportIndex::default();
    index.update(report, view, None, None);
    index
}

#[test]
fn source_groups_share_bytes_but_keep_physical_aliases_and_complete_rows() {
    let mut report = native();
    let mut alias = report.programs[0].as_ref().clone();
    alias.id = report.programs.len();
    alias.code_address = Some(format!("0x{}02", "00".repeat(19)));
    report.programs.push(Arc::new(alias));
    let mut different = report.programs[0].as_ref().clone();
    different.id = report.programs.len();
    different.kind = api::CodeKind::Initcode;
    report.programs.push(Arc::new(different));
    let mut different = report.programs[0].as_ref().clone();
    different.id = report.programs.len();
    different.bytecode.push_str("00");
    report.programs.push(Arc::new(different));
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native);
    let index = index(&report, view);
    assert_eq!(
        index
            .code_groups()
            .iter()
            .map(|group| group.programs.as_slice())
            .collect::<Vec<_>>(),
        vec![&[0, 1][..], &[2][..], &[3][..]]
    );
    let presentation = Presentation::new(&report, &index, view, None);
    let focus = presentation
        .graph_focus(
            view,
            Selection {
                state: Some(0),
                ..Default::default()
            },
            Some(0),
        )
        .unwrap();
    let mut wrong_alias = focus.clone();
    wrong_alias.source = presentation.source(1);
    assert_eq!(
        presentation.validate(&wrong_alias),
        Err(NavigationError::SourceMismatch)
    );
    assert_ne!(presentation.source(0), presentation.source(1));

    let program = presentation.program(0).unwrap();
    let rows = index.source_rows(0).unwrap();
    assert_eq!(
        rows.len(),
        program
            .blocks
            .iter()
            .map(|block| block.instructions.len() + 1)
            .sum::<usize>()
    );
    assert!(
        matches!(rows.row(program, 0), Some(SourceRow::Block(block)) if block.id == program.blocks[0].id)
    );
    assert!(
        matches!(rows.row(program, 1), Some(SourceRow::Instruction { block, instruction }) if block.id == program.blocks[0].id && instruction.pc == program.blocks[0].instructions[0].pc)
    );
    assert_eq!(rows.block_row(0), Some(0));
    assert_eq!(rows.block_row(usize::MAX), None);
    assert!(rows.row(program, rows.len()).is_none());
}

#[test]
fn decoded_source_pc_never_creates_a_native_instruction_receipt() {
    let mut report = native();
    let graph = Arc::make_mut(report.native.as_mut().unwrap());
    graph.ssa.blocks[0].coverage = api::BlockCoverage::Unexecuted;
    graph.ssa.blocks[0].instructions.clear();
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native);
    let index = index(&report, view);
    let presentation = Presentation::new(&report, &index, view, None);
    let mut focus = presentation
        .focus_context(ContextTarget {
            view,
            context: ContextFocus::Graph { state: 0 },
        })
        .unwrap();
    focus.evidence = EvidenceFocus::Source {
        block: Some(0),
        pc: Some(0),
    };
    assert!(presentation.validate(&focus).is_ok());
    assert!(focus.graph_selection().is_none());
    assert_eq!(
        presentation.capabilities(&focus).unwrap().ssa,
        Capability::Unavailable(UnavailableReason::Unexecuted)
    );
    assert_eq!(
        presentation.graph_focus(
            view,
            Selection {
                state: Some(0),
                pc: Some(0),
                value: None
            },
            Some(0)
        ),
        Err(NavigationError::MissingEvidence)
    );
}

#[test]
fn source_and_state_evidence_are_validated_in_their_original_context() {
    let report = native();
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native);
    let index = index(&report, view);
    let presentation = Presentation::new(&report, &index, view, None);
    let focus = presentation
        .graph_focus(
            view,
            Selection {
                state: Some(0),
                pc: Some(0),
                value: None,
            },
            Some(0),
        )
        .unwrap();
    let mut invalid = focus.clone();
    invalid.context = Some(ContextFocus::Graph { state: 1 });
    assert_eq!(
        presentation.validate(&invalid),
        Err(NavigationError::ContextMismatch)
    );
    invalid = focus.clone();
    invalid.evidence = EvidenceFocus::Source {
        block: Some(0),
        pc: Some(5),
    };
    assert_eq!(
        presentation.validate(&invalid),
        Err(NavigationError::MissingEvidence)
    );
    invalid = focus;
    invalid.source = Some(SourceFocus::Captured(SourceId {
        report: ReportViewId::fixture(9, api::AnalysisNamespace::Native).generation(),
        program: 0,
    }));
    assert_eq!(
        presentation.validate(&invalid),
        Err(NavigationError::StaleReport)
    );
    assert_eq!(
        presentation.graph_focus(view, Selection::default(), Some(usize::MAX)),
        Err(NavigationError::MissingSource)
    );
}

#[test]
fn conservative_focus_keeps_actual_owner_source_and_static_qualifications() {
    let report = process();
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Baseline).minimum(None);
    let index = index(report, view);
    let presentation = Presentation::new(report, &index, view, None);
    let focus = presentation.entry_focus(view).unwrap();
    let Some(ResolvedContext::Minimum {
        procedure,
        source_status,
        ..
    }) = presentation.resolve_context(&focus).unwrap()
    else {
        panic!("minimum context")
    };
    assert_eq!(source_status, api::SummarySourceStatus::Current);
    let frame = procedure.process.as_ref().unwrap();
    assert_eq!(frame.owner, api::ProcessOwner::RootNamespace);
    assert!(!frame.is_static);
    let mut invalid = focus.clone();
    invalid.evidence = EvidenceFocus::Graph(Selection {
        state: Some(0),
        pc: Some(0),
        value: None,
    });
    assert_eq!(
        presentation.validate(&invalid),
        Err(NavigationError::ContextMismatch)
    );
    let capabilities = presentation.capabilities(&focus).unwrap();
    assert_eq!(capabilities.cfg, Capability::Available);
    assert_eq!(
        capabilities.ssa,
        Capability::Unavailable(UnavailableReason::ConservativeEvidence)
    );

    let other = index
        .contexts()
        .iter()
        .copied()
        .find(|target| {
            matches!(target.context, ContextFocus::Minimum { .. })
                && target.context != focus.context.unwrap()
        })
        .expect("unknown external call produces another qualified procedure");
    let mut invalid = focus.clone();
    invalid.context = Some(other.context);
    assert!(matches!(
        presentation.validate(&invalid),
        Err(NavigationError::SourceMismatch | NavigationError::ContextMismatch)
    ));
    let unknown = presentation.focus_context(other).unwrap();
    assert!(presentation.resolve_context(&unknown).unwrap().is_some());
    assert!(unknown.graph_selection().is_none());
}

#[test]
fn publication_and_model_versions_are_independent_cache_axes() {
    let report = process();
    let v = |value| api::PublicationVersion(api::DecimalU64(value));
    let mut publication = JobPublication {
        version: v(10),
        minimum_version: Some(v(2)),
        global_version: Some(v(3)),
        native_version: Some(v(4)),
        model_closed: true,
    };
    let view = ReportViewId::fixture(1, api::AnalysisNamespace::Baseline)
        .minimum(publication.minimum_version);
    let mut index = ReportIndex::default();
    index.update(report, view, Some(&publication), Some(JobId(77)));
    let source_rows = index.source_rows(0).unwrap() as *const ProgramRows;
    let presentation = Presentation::new(report, &index, view, Some(&publication));
    let focus = presentation.entry_focus(view).unwrap();
    let identity = presentation.identity();
    assert_eq!(identity.job, Some(JobId(77)));
    assert_eq!(identity.publication, Some(v(10)));
    assert_eq!(identity.minimum, Some(v(2)));
    assert_ne!(identity.minimum, identity.global);
    index.update(
        report,
        view.for_namespace(api::AnalysisNamespace::Native, Some(&publication)),
        Some(&publication),
        Some(JobId(77)),
    );
    assert_eq!(
        source_rows,
        index.source_rows(0).unwrap() as *const ProgramRows,
        "namespace-only changes reuse source offsets"
    );
    publication.version = v(11);
    publication.native_version = Some(v(11));
    index.update(report, view, Some(&publication), Some(JobId(77)));
    let presentation = Presentation::new(report, &index, view, Some(&publication));
    assert!(
        presentation.validate(&focus).is_ok(),
        "optional Native arrival retains unchanged Global focus"
    );
    let mut wrong = focus.clone();
    wrong.view = view.minimum(Some(v(11)));
    assert_eq!(
        presentation.validate(&wrong),
        Err(NavigationError::StaleView)
    );
    publication.minimum_version = Some(v(12));
    index.update(report, view, Some(&publication), Some(JobId(77)));
    assert_eq!(
        Presentation::new(report, &index, view, Some(&publication)).validate(&focus),
        Err(NavigationError::StaleView)
    );
}

#[test]
fn retained_ssa_coverage_is_resolved_without_claiming_current_execution() {
    for coverage in [
        api::BlockCoverage::Current,
        api::BlockCoverage::Stale,
        api::BlockCoverage::Unexecuted,
    ] {
        let mut report = native();
        let block = &mut Arc::make_mut(report.native.as_mut().unwrap()).ssa.blocks[0];
        block.coverage = coverage;
        if coverage != api::BlockCoverage::Current {
            block.instructions.clear();
        }
        let view = ReportViewId::fixture(1, api::AnalysisNamespace::Native);
        let index = index(&report, view);
        let presentation = Presentation::new(&report, &index, view, None);
        let focus = presentation.entry_focus(view).unwrap();
        let Some(ResolvedContext::Graph {
            coverage: resolved,
            at_current_revision,
            ..
        }) = presentation.resolve_context(&focus).unwrap()
        else {
            panic!("Native context")
        };
        assert_eq!(resolved, Some(coverage));
        assert!(at_current_revision);
        let expected = match coverage {
            api::BlockCoverage::Current => Capability::Available,
            api::BlockCoverage::Stale => {
                Capability::Unavailable(UnavailableReason::HistoricalReceipt)
            }
            api::BlockCoverage::Unexecuted => {
                Capability::Unavailable(UnavailableReason::Unexecuted)
            }
        };
        assert_eq!(presentation.capabilities(&focus).unwrap().ssa, expected);
    }
}
