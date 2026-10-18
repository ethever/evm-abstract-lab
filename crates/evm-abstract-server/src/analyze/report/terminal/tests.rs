//! Source regressions for real native attachment and the full wire boundary.
use super::*;
use crate::analyze::{input, report};
use alloy_primitives::{Address, B256, U256};
use evm_abstract::{
    analysis::{WorldAnalysis, control::Control},
    domain::AbstractValue,
    world::{Account, Entry, Store, World},
};

// Reuse the core's two-block cap fixture through public execution APIs. The
// transfer cap interrupts actual detail; no certificate is minted by the test.
const TWO_BLOCK_RETURN: &str = "6003565b602a5f5260205ff3";

fn fixture(enabled: bool, is_static: bool, chain: bool) -> (WorldAnalysis, api::AnalyzeRequest) {
    let owner = Address::repeat_byte(0x11);
    let other = Address::repeat_byte(0x22);
    let mut request = api::AnalyzeRequest::default();
    request.input = api::AnalysisInput::Bytecode(api::BytecodeInput {
        bytecode: TWO_BLOCK_RETURN.into(),
        address: None,
    });
    request.limits.max_transfers = 1;
    request.limits.root_terminal_coverage = enabled;
    request.environment.is_static = is_static;
    let mut world = if chain {
        World::anchored(
            Fork::Osaka,
            U256::MAX,
            B256::repeat_byte(0x45),
            "typed wire binding",
        )
    } else {
        World::offline(Fork::Osaka, "same offline label", "typed wire binding")
    };
    for (address, code, stored) in [(owner, TWO_BLOCK_RETURN, 7_u64), (other, "00", 9)] {
        let mut account = Account::from_hex(code, Fork::Osaka).unwrap();
        account
            .storage
            .insert(U256::ZERO, AbstractValue::constant(U256::from(stored)));
        account.balance = AbstractValue::constant(U256::from(10));
        account.nonce = AbstractValue::constant(U256::ONE);
        world.insert(address, account).unwrap();
    }
    let native = analysis::analyze_world(
        world,
        Entry {
            address: owner,
            environment: input::environment(&request.environment).unwrap(),
        },
        input::config(&request.limits).unwrap(),
    )
    .unwrap();
    (native, request)
}
fn project(native: &WorldAnalysis, request: &api::AnalyzeRequest) -> api::AnalysisReport {
    let projected = report::report(
        native,
        request,
        api::AnalysisScope::SingleProgram,
        &Control::default(),
    )
    .unwrap();
    projected.validate().unwrap();
    let encoded = serde_json::to_vec(&projected).unwrap();
    let decoded: api::AnalysisReport = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, projected);
    decoded.validate().unwrap();
    decoded
}
fn scalar(projected: &api::ValueInfo, native: &AbstractValue) {
    let mut numerical = projected.clone();
    numerical.expression = None;
    assert_eq!(numerical, value::info(native));
    assert_eq!(
        projected.expression.is_some(),
        native.expression().is_some()
    );
}

#[test]
fn default_off_and_opt_in_none_preserve_honest_detail() {
    assert!(!api::AnalysisLimits::default().root_terminal_coverage);
    assert!(
        !input::config(&api::AnalysisLimits::default())
            .unwrap()
            .root_terminal_coverage
    );
    let (ordinary, request) = fixture(false, false, false);
    let ordinary = project(&ordinary, &request);
    assert_eq!(ordinary.status, api::AnalysisStatus::Incomplete);
    assert!(ordinary.root_terminal_coverage.is_none());
    let encoded = serde_json::to_string(&ordinary).unwrap();
    let missing = encoded.replace("\"root_terminal_coverage\":null,", "");
    assert_ne!(encoded, missing);
    assert!(
        serde_json::from_str::<api::AnalysisReport>(&missing)
            .unwrap_err()
            .to_string()
            .contains("missing field: root_terminal_coverage")
    );
    assert!(
        ordinary
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == api::FrontierKind::Transfers)
    );
    let mut request = api::AnalyzeRequest::default();
    request.limits.root_terminal_coverage = true;
    // A converged real program has no triggering refusal and no certificate.
    let complete = crate::analyze::analyze(request).unwrap();
    assert_eq!(complete.status, api::AnalysisStatus::Converged);
    assert!(complete.root_terminal_coverage.is_none());
    complete.validate().unwrap();
}

#[test]
fn incomplete_detail_and_all_three_joint_tuples_keep_the_actual_original_input() {
    let (native, request) = fixture(true, false, false);
    let report = project(&native, &request);
    assert_eq!(report.schema_version, 15);
    assert_eq!(report.status, api::AnalysisStatus::Incomplete);
    assert!(!report.ssa.complete);
    assert_eq!(report.cfg.len(), native.states().len());
    assert_eq!(report.frontiers.len(), native.frontiers().len());
    assert_eq!(report.outcomes.len(), native.outcomes().len());
    assert_eq!(report.states[0].state, native.states()[0].id);
    assert_eq!(report.cfg[0].executed_pcs, native.states()[0].executed_pcs);
    let coverage = report.root_terminal_coverage.as_ref().unwrap();
    let source = native.root_terminal_coverage().unwrap();
    assert_eq!(coverage.scope, api::RootTerminalScope::AllInputStates);
    assert_eq!(coverage.trigger, api::RootTerminalTrigger::Transfers);
    assert_eq!(
        coverage.outcomes.each_ref().map(|outcome| outcome.kind),
        [
            api::OutcomeKind::Return,
            api::OutcomeKind::Revert,
            api::OutcomeKind::Failure
        ]
    );
    let binding = &coverage.input;
    let frame = &report.states[0].entry.frames[0];
    assert_eq!(binding.program, frame.program.unwrap());
    assert_eq!(binding.calldata, frame.calldata);
    assert_eq!(binding.call_value, frame.call_value);
    assert_eq!(binding.saved_store, frame.rollback_store);
    assert_eq!(binding.initial_store, report.states[0].entry.store);
    assert_eq!(binding.environment, report.metadata.environment);
    assert_eq!(
        binding.frame.address_value,
        api::AddressValue::Symbolic(value::symbol(evm_abstract::domain::identity::Symbol::To))
    );
    assert_eq!(
        binding.frame.stack_height,
        source.input().frame().stack_height
    );
    assert_eq!(
        binding.revision,
        api::DecimalU64(source.input().revision().value())
    );
    assert_eq!(
        binding.input_scope,
        source.input().input_scope().map(api::DecimalU64)
    );
    scalar(&binding.call_value, source.input().call_value());
    let identity = binding.call_value.identity.as_ref().unwrap();
    assert_eq!(Some(identity.scope), binding.input_scope);
    assert_eq!(
        report.symbolic.nodes[binding.call_value.expression.unwrap().node],
        api::Expression::Input(identity.clone())
    );
    let api::RootTerminalSnapshot::Offline(snapshot) = &binding.snapshot else {
        panic!("offline snapshot")
    };
    assert_eq!(
        Some(snapshot.namespace.0),
        source.input().snapshot().offline_namespace()
    );
    let domain = source.input().domain_spec();
    assert_eq!(binding.domain_spec.schema_version, domain.schema_version());
    assert_eq!(binding.domain_spec.word_bits, domain.word_bits());
    assert_eq!(binding.domain_spec.cost_version, domain.cost_version());
    assert_eq!(
        binding.domain_spec.provenance_policy,
        domain.provenance_policy()
    );
    for (outcome, source) in coverage.outcomes.iter().zip(source.outcomes()) {
        let data = &report.byte_arrays[outcome.data];
        scalar(&data.length, source.data().len());
        scalar(&data.default, source.data().default_byte());
        let store = &report.stores[outcome.store];
        scalar(&store.nonce_default, &source.store().nonce_default());
        scalar(&store.balance_default, source.store().balance_default());
        scalar(
            &store.storage_default,
            source.store().global_storage_default(),
        );
        scalar(
            &store.transient_default,
            source.store().global_transient_default(),
        );
        assert_eq!(store.created_default, source.store().created_default());
        assert_eq!(
            store.pending_destruction_default,
            source.store().pending_destruction_default()
        );
        assert_eq!(store.touched_default, source.store().touched_default());
    }
    let returned = &report.stores[coverage.outcomes[0].store];
    assert!(returned.code_default.is_none());
    assert_eq!(returned.existence_default, api::AccountExistence::Unknown);
    assert!(returned.accounts.is_empty());
    assert_ne!(
        returned.storage_root,
        report.stores[binding.initial_store].storage_root
    );
    assert_eq!(
        report.stores[coverage.outcomes[1].store],
        report.stores[coverage.outcomes[2].store]
    );
    let failure = &report.byte_arrays[coverage.outcomes[2].data];
    assert_eq!(failure.length.unsigned.upper, "0x0");
    for outcome in &coverage.outcomes[..2] {
        let data = &report.byte_arrays[outcome.data];
        assert_eq!(data.length.unsigned.upper, value::word(U256::MAX));
        assert!(data.cells.is_empty());
        let api::Expression::View(view) = &report.symbolic.nodes[data.symbolic.node] else {
            panic!("complete view")
        };
        assert!(matches!(
            report.symbolic.nodes[view.state.node],
            api::Expression::BytesInput(_)
        ));
    }
}

#[test]
fn chain_identity_and_static_nonselected_account_state_survive_projection() {
    let (native, request) = fixture(true, true, true);
    let report = project(&native, &request);
    let coverage = report.root_terminal_coverage.as_ref().unwrap();
    let api::RootTerminalSnapshot::Chain(snapshot) = &coverage.input.snapshot else {
        panic!("chain snapshot")
    };
    assert_eq!(snapshot.chain_id, value::word(U256::MAX));
    assert_eq!(snapshot.block_hash, B256::repeat_byte(0x45).to_string());
    let saved = &report.stores[coverage.input.saved_store];
    let initial = &report.stores[coverage.input.initial_store];
    let returned = &report.stores[coverage.outcomes[0].store];
    let reverted = &report.stores[coverage.outcomes[1].store];
    for store in [saved, initial, returned, reverted] {
        let other = store
            .accounts
            .iter()
            .find(|account| account.address == Address::repeat_byte(0x22).to_string())
            .unwrap();
        assert_eq!(other.nonce.constants, Some(vec!["0x1".into()]));
        assert_eq!(other.balance.constants, Some(vec!["0xa".into()]));
        assert_eq!(other.storage[0].value.constants, Some(vec!["0x9".into()]));
        assert_eq!(other.code_kind, api::CodeKind::Runtime);
        assert_eq!(store.storage_root, saved.storage_root);
    }
    for (before, after) in [(initial, returned), (saved, reverted)] {
        for account in &before.accounts {
            let mut expected = account.clone();
            expected.created = Some(false);
            expected.pending_destruction = Some(false);
            expected.touched = Some(false);
            assert_eq!(
                after
                    .accounts
                    .iter()
                    .find(|account| account.address == expected.address),
                Some(&expected)
            );
        }
    }
    let mut invalid = report.clone();
    let api::RootTerminalSnapshot::Chain(snapshot) = &mut invalid
        .root_terminal_coverage
        .as_mut()
        .unwrap()
        .input
        .snapshot
    else {
        unreachable!()
    };
    snapshot.chain_id = "0x1".into();
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::RootTerminal(
            api::RootTerminalValidationFailure::Snapshot
        ))
    );
}

#[test]
fn ordinary_stores_export_unlisted_defaults_and_actual_unscoped_templates() {
    let owner = Address::repeat_byte(0x11);
    let other = Address::repeat_byte(0x22);
    let unlisted = Address::repeat_byte(0x33);
    let mut world = World::offline(Fork::Osaka, "ordinary roots", "wire fixture");
    world.insert(owner, Account::unknown()).unwrap();
    world.insert(other, Account::unknown()).unwrap();
    let store = Store::new(&world);
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let id = pools.store(&store).unwrap();
    let snapshot = pools.stores[id].clone();
    assert!(
        !snapshot
            .accounts
            .iter()
            .any(|account| account.address == unlisted.to_string())
    );
    assert_eq!(
        snapshot.storage_root,
        pools.symbolic.slots(store.global_storage_state()).unwrap()
    );
    assert_eq!(
        snapshot.transient_root,
        pools
            .symbolic
            .slots(store.global_transient_state())
            .unwrap()
    );
    assert!(snapshot.code_default.is_none());
    assert_eq!(snapshot.existence_default, api::AccountExistence::Unknown);
    assert_eq!(snapshot.created_default, Some(false));
    assert_eq!(snapshot.pending_destruction_default, Some(false));
    scalar(&snapshot.nonce_default, &store.nonce(unlisted));
    let first = snapshot
        .accounts
        .iter()
        .find(|account| account.address == owner.to_string())
        .unwrap();
    let second = snapshot
        .accounts
        .iter()
        .find(|account| account.address == other.to_string())
        .unwrap();
    assert_ne!(first.storage_root, second.storage_root);
    assert_ne!(first.storage_root, snapshot.storage_root);
    pools.symbolic.graph.validate().unwrap();
    let (native, request) = fixture(false, false, false);
    let report = project(&native, &request);
    let mut invalid = report.clone();
    invalid.stores[0].storage_root.node = report.symbolic.nodes.len();
    assert!(matches!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Symbolic(_))
    ));
    invalid = report;
    invalid.stores[0].transient_root.node = invalid
        .metadata
        .environment
        .call_value
        .expression
        .map_or(usize::MAX, |expression| expression.node);
    assert!(matches!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Symbolic(_))
    ));
}

#[test]
fn references_order_empty_failure_and_binding_mismatches_are_rejected() {
    let (native, request) = fixture(true, false, false);
    let report = project(&native, &request);
    for (which, expected) in [
        (0, api::PoolReferenceKind::ByteArray),
        (1, api::PoolReferenceKind::Store),
        (2, api::PoolReferenceKind::Program),
    ] {
        let mut invalid = report.clone();
        let input = &mut invalid.root_terminal_coverage.as_mut().unwrap().input;
        match which {
            0 => input.calldata = usize::MAX,
            1 => input.saved_store = usize::MAX,
            _ => input.program = usize::MAX,
        }
        assert!(
            matches!(invalid.validate(), Err(api::ReportValidationFailure::Pool(api::PoolReferenceFailure { kind, .. })) if kind == expected)
        );
    }
    let mut invalid = report.clone();
    invalid
        .root_terminal_coverage
        .as_mut()
        .unwrap()
        .input
        .storage_source
        .node = report.symbolic.nodes.len();
    assert!(matches!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Symbolic(_))
    ));
    let mut invalid = report.clone();
    invalid
        .root_terminal_coverage
        .as_mut()
        .unwrap()
        .input
        .call_value
        .identity
        .as_mut()
        .unwrap()
        .scope = api::DecimalU64(u64::MAX);
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::RootTerminal(
            api::RootTerminalValidationFailure::InputIdentity
        ))
    );
    let mut invalid = report.clone();
    invalid
        .root_terminal_coverage
        .as_mut()
        .unwrap()
        .outcomes
        .swap(0, 1);
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::RootTerminal(
            api::RootTerminalValidationFailure::OutcomeOrder
        ))
    );
    let mut invalid = report.clone();
    let outcomes = &mut invalid.root_terminal_coverage.as_mut().unwrap().outcomes;
    outcomes[2].data = outcomes[0].data;
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::RootTerminal(
            api::RootTerminalValidationFailure::FailureData
        ))
    );
    for kind in [
        api::RootTerminalValidationFailure::Fork,
        api::RootTerminalValidationFailure::Frame,
        api::RootTerminalValidationFailure::Environment,
        api::RootTerminalValidationFailure::Configuration,
        api::RootTerminalValidationFailure::DomainSpec,
        api::RootTerminalValidationFailure::InputIdentity,
    ] {
        let mut invalid = report.clone();
        let input = &mut invalid.root_terminal_coverage.as_mut().unwrap().input;
        match kind {
            api::RootTerminalValidationFailure::Fork => input.fork = api::Fork::Cancun,
            api::RootTerminalValidationFailure::Frame => {
                input.frame.code_hash = B256::ZERO.to_string()
            }
            api::RootTerminalValidationFailure::Environment => {
                input.environment.is_static = !input.environment.is_static
            }
            api::RootTerminalValidationFailure::Configuration => input.config.max_call_depth += 1,
            api::RootTerminalValidationFailure::DomainSpec => input.domain_spec.capacity += 1,
            api::RootTerminalValidationFailure::InputIdentity => {
                input.input_scope = Some(api::DecimalU64(u64::MAX))
            }
            _ => unreachable!(),
        }
        assert_eq!(
            invalid.validate(),
            Err(api::ReportValidationFailure::RootTerminal(kind))
        );
    }
    let mut invalid = report;
    invalid.schema_version = 14;
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Schema(14))
    );
}

#[test]
fn projection_errors_and_cancellation_return_no_partial_report_or_certificate() {
    let (native, request) = fixture(true, false, false);
    let source = native.root_terminal_coverage().unwrap();
    for kind in [
        api::ExportFailureKind::Nodes,
        api::ExportFailureKind::Depth,
        api::ExportFailureKind::ExactBytes,
    ] {
        let control = Control::default();
        let mut pools = Pools::new(&control);
        match kind {
            api::ExportFailureKind::Nodes => pools.symbolic.limits.max_nodes = 0,
            api::ExportFailureKind::Depth => pools.symbolic.limits.max_depth = 0,
            api::ExportFailureKind::ExactBytes => pools.symbolic.limits.max_exact_bytes = 0,
            _ => unreachable!(),
        }
        let error = super::project(source, &mut pools).unwrap_err();
        assert!(matches!(error.details, api::ErrorDetails::Export(detail) if detail.kind == kind));
    }
    let control = Control::new(Default::default());
    control.cancellation().cancel();
    let error = report::report(
        &native,
        &request,
        api::AnalysisScope::SingleProgram,
        &control,
    )
    .unwrap_err();
    assert_eq!(error.code, api::ApiErrorCode::Cancelled);
    let mut pools = Pools::new(&control);
    assert!(super::project(source, &mut pools).is_err());
    assert!(pools.symbolic.graph.nodes.is_empty());
    assert!(pools.stores.is_empty());
    // The same immutable native record remains projectable after a failed export.
    project(&native, &request).validate().unwrap();
}
