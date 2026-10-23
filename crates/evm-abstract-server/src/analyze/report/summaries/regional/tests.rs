use super::*;
use evm_abstract::analysis::control::Control;

#[test]
fn regional_projection_is_paid_and_checks_cancellation_even_for_empty_evidence() {
    let control = Control::new(Default::default());
    let mut export = Pool::new(&control);
    let before = export.usage.work;
    let edge = analysis::EvmRegionalEdge {
        to: 0,
        kind: analysis::MachineEdgeKind::Summary(analysis::OutcomeKind::Revert),
    };
    assert_eq!(edges(&[edge, edge], &mut export).unwrap().len(), 2);
    assert_eq!(export.usage.work - before, 4);
    let receipt = analysis::EvmProcessTransfer {
        repeat: true,
        actions: vec![analysis::ProcessAction::StaticCall],
        unresolved: vec![analysis::ProcessBoundary::ResidualOwner(
            analysis::ProcessAction::StaticCall,
        )],
    };
    let before = export.usage.work;
    process::transfer(&receipt, &mut export).unwrap();
    assert_eq!(export.usage.work - before, 5);
    let cfg = analysis::EvmRegionalCfg::default();
    assert_eq!(
        project(&cfg, &[], &mut export).unwrap(),
        api::RegionalCfg::default()
    );
    control.cancellation().cancel();
    assert_eq!(
        process::transfer(&receipt, &mut export).unwrap_err().code,
        api::ApiErrorCode::Cancelled
    );
    let error = project(&cfg, &[], &mut export).unwrap_err();
    assert_eq!(error.code, api::ApiErrorCode::Cancelled);
    assert!(
        matches!(error.details, api::ErrorDetails::Export(detail) if detail.kind == api::ExportFailureKind::Cancelled)
    );
}
