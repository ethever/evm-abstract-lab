//! Offline native-to-wire integration; production default policy is unchanged.
use super::*;
use evm_abstract::{
    U256,
    analysis::{
        CorrelatedJoin, FeedbackAcceleration, RegionalSummaries, SemanticPolicies,
        StructuralPartition,
    },
    render,
};

fn address(id: u64) -> Address {
    let mut bytes = [0u8; 20];
    bytes[12..].copy_from_slice(&id.to_be_bytes());
    Address::from(bytes)
}

fn fixture(
    accounts: &[(u64, &str, bool)],
) -> (
    analysis::EvmWorldAnalysis<NumericValue>,
    api::AnalysisReport,
) {
    let result = fixture_with_policy(accounts, RegionalSummaries::with_early_known_admission());
    assert_eq!(
        result.1.status,
        api::AnalysisStatus::Converged,
        "{:?}",
        result.1.frontiers
    );
    result
}

fn fixture_with_policy<S: analysis::ProcedureSummaryPolicy<ProductAlgebra>>(
    accounts: &[(u64, &str, bool)],
    summary_policy: S,
) -> (
    analysis::EvmWorldAnalysis<NumericValue>,
    api::AnalysisReport,
) {
    let mut world = EvmWorld::new(Fork::Osaka, "offline regional consumer fixture");
    for &(id, code, storage_unknown) in accounts {
        let mut account = EvmAccount::from_hex(code, Fork::Osaka).unwrap();
        account.storage_unknown = storage_unknown;
        account.balance = evm_abstract::domain::EvmValue::constant(U256::from(10));
        world.insert(address(id), account).unwrap();
    }
    let config = analysis::ExecutionConfig {
        root_terminal_coverage: false,
        // The CALL-family fixture checks application projection and deliveries.
        // Use the request allowance rather than the smaller bare-engine default.
        max_work: usize::try_from(api::AnalysisLimits::default().max_work).unwrap(),
        ..Default::default()
    };
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()));
    let policies = EnginePolicies::default().with_state(SemanticPolicies::new(
        StructuralPartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        summary_policy,
    ));
    let mut extension = NoExtension;
    let mut engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
    // CALLER, value and calldata keep their unspecified domains.
    let native = engine
        .run(
            AnalysisInput::new(world, EvmEntry::new(address(0x101))).unwrap(),
            config,
            &Control::default(),
        )
        .unwrap();
    eprintln!(
        "regional fixture: status={:?}, work={}, transfers={}, frontiers={}",
        native.status(),
        native.work(),
        native.transfers(),
        native.frontiers().len()
    );
    let mut request = api::AnalyzeRequest::default();
    request.limits.root_terminal_coverage = false;
    let report = report::report(
        &native,
        &request,
        api::AnalysisScope::RpcWorld,
        &algebra,
        api::NumericModel::Product,
        &Control::default(),
    )
    .unwrap();
    assert_eq!(report.schema_version, 19);
    assert!(report.root_terminal_coverage.is_none());
    report.validate().unwrap();
    (native, report)
}

const ROOT: &str = "5f5f5f5f5f6101025af15000";

#[test]
fn known_process_identity_actions_exits_and_unexpanded_rules_survive_native_to_wire() {
    for code in ["5b5f56", "5f5f5f5f6101025afa5000"] {
        let (native, report) = fixture_with_policy(
            &[(0x101, code, false), (0x102, "00", false)],
            analysis::KnownProcessSummaries,
        );
        let raw = serde_json::to_value(render::world::evidence(&native).unwrap()).unwrap();
        assert_eq!(raw["schema_version"], 14);
        assert_eq!(raw["domain_spec"]["schema_version"], 3);
        assert_eq!(raw["domain_spec"]["cost_version"], 8);
        assert!(report.root_terminal_coverage.is_none());
        assert!(!report.abstract_summaries.is_empty());
        for (record, mapped) in native
            .procedure_summaries()
            .iter()
            .zip(&report.abstract_summaries)
        {
            assert_eq!(mapped.kind, api::ProcedureSummaryKind::KnownProcess);
            assert_eq!(mapped.source_current, record.source_current);
            assert_eq!(mapped.closed, record.closed);
            for (procedure, actual) in record.procedures.iter().zip(&mapped.procedures) {
                assert_eq!(actual.semantics, process::semantics(procedure.semantics));
                assert_eq!(actual.kind, pools::mode(procedure.frame.mode));
                assert_eq!(
                    actual.code_hash,
                    Some(procedure.frame.code_hash.to_string())
                );
                if actual.semantics == api::TransferSemantics::KnownProcess {
                    assert!(actual.covered_blocks.is_empty());
                }
            }
            for (node, actual) in record
                .regional_cfg
                .nodes
                .iter()
                .zip(&mapped.regional_cfg.nodes)
            {
                if mapped.procedures[actual.procedure].semantics
                    == api::TransferSemantics::KnownProcess
                {
                    assert_eq!(actual.frame.kind, api::CodeKind::Runtime);
                    assert_eq!(actual.frame.basic_block, None);
                }
                if let Some(receipt) = &node.transfer {
                    let actual = actual.transfer.as_ref().unwrap();
                    assert_eq!(actual.semantics, process::semantics(receipt.semantics));
                    assert_eq!(
                        serde_json::to_value(&actual.process).unwrap(),
                        serde_json::to_value(&receipt.process).unwrap()
                    );
                    assert_eq!(actual.visited_block, receipt.visited_block);
                    assert!(!actual.visited_block);
                    assert_eq!(
                        serde_json::to_value(&actual.outcomes).unwrap(),
                        serde_json::to_value(&receipt.outcomes).unwrap()
                    );
                }
            }
            for (call, actual) in record
                .regional_cfg
                .calls
                .iter()
                .zip(&mapped.regional_cfg.calls)
            {
                assert_eq!(actual.action, call.action.map(process::action));
                assert_eq!(actual.target, call.target.map(process::owner));
            }
        }
        if code.contains("fa") {
            assert!(report.abstract_summaries.iter().any(|summary| {
                summary.regional_cfg.calls.iter().any(|call| {
                    call.action == Some(api::ProcessAction::StaticCall)
                        && call.target == Some(api::ProcessOwner::Named(address(0x102).to_string()))
                        && call
                            .entries
                            .iter()
                            .any(|id| summary.regional_cfg.nodes[*id].frame.is_static)
                })
            }));
            assert!(report.frontiers.iter().any(|frontier| matches!(
                frontier.reason,
                api::FrontierDetails::Process(api::ProcessBoundary::ResidualOwner(
                    api::ProcessAction::StaticCall
                ))
            )));
        }
        let decoded: api::AnalyzeReply = serde_json::from_slice(
            &serde_json::to_vec(&api::AnalyzeReply {
                result: Ok(report.clone()),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded.result.unwrap(), report);
        let mut malformed = report;
        let summary = malformed
            .abstract_summaries
            .iter_mut()
            .find(|summary| {
                summary
                    .regional_cfg
                    .nodes
                    .iter()
                    .any(|node| node.transfer.is_some())
            })
            .unwrap();
        summary
            .regional_cfg
            .nodes
            .iter_mut()
            .find_map(|node| node.transfer.as_mut())
            .unwrap()
            .visited_block = true;
        assert!(matches!(
            malformed.validate(),
            Err(api::ReportValidationFailure::Regional(
                api::RegionalValidationFailure {
                    kind: api::RegionalValidationKind::BlockVisit,
                    ..
                }
            ))
        ));
    }
}

fn calls_fixture() -> (
    analysis::EvmWorldAnalysis<NumericValue>,
    api::AnalysisReport,
) {
    fixture(&[
        (0x101, ROOT, false),
        (
            0x102,
            concat!(
                "5f5f5f5f5f6101035af150",   // CALL
                "5f5f5f5f6101035af450",     // DELEGATECALL
                "5f5f5f5f60036101035af250", // CALLCODE
                "5f5f5f5f6101035afa5000",   // STATICCALL
            ),
            false,
        ),
        (0x103, "00", false),
    ])
}

fn assert_mapping(native: &analysis::EvmWorldAnalysis<NumericValue>, report: &api::AnalysisReport) {
    assert_eq!(
        native.procedure_summaries().len(),
        report.abstract_summaries.len()
    );
    for (source, summary) in native
        .procedure_summaries()
        .iter()
        .zip(&report.abstract_summaries)
    {
        assert_eq!(summary.kind, api::ProcedureSummaryKind::KnownCall);
        assert_eq!(source.kind, analysis::ProcedureSummaryKind::KnownCall);
        assert_eq!(summary.closed, source.closed);
        assert_eq!(summary.source_state, source.source_state);
        assert_eq!(summary.source_current, source.source_current);
        assert_eq!(summary.root_procedure, source.root_procedure);
        let cfg = &summary.regional_cfg;
        assert_eq!(cfg.nodes.len(), source.regional_cfg.nodes.len());
        assert_eq!(cfg.calls.len(), source.regional_cfg.calls.len());
        for (node, mapped) in source.regional_cfg.nodes.iter().zip(&cfg.nodes) {
            assert_eq!(
                (
                    mapped.id,
                    mapped.procedure,
                    mapped.input_revision.0,
                    mapped.excluded
                ),
                (node.id, node.procedure, node.input_revision, node.excluded)
            );
            assert_eq!(
                mapped.frame.code_address,
                node.frame.code_address.to_string()
            );
            assert_eq!(
                mapped.frame.code_hash,
                (node.frame.mode != analysis::FrameCode::Unknown)
                    .then(|| node.frame.code_hash.to_string())
            );
            assert_eq!(mapped.frame.storage_address, node.frame.address.to_string());
            assert_eq!(
                mapped.frame.address_value,
                value::address(node.frame.address_value)
            );
            assert_eq!(mapped.frame.caller, value::address(node.frame.caller));
            assert_eq!(mapped.frame.is_static, node.frame.is_static);
            assert_eq!(mapped.frame.kind, pools::mode(node.frame.mode));
            assert_eq!(mapped.frame.context, node.frame.jump_history);
            assert_eq!(mapped.frame.stack_height, node.frame.stack_height);
            assert_eq!(mapped.successors, node.successors);
            if let Some(transfer) = &node.transfer {
                let actual = mapped.transfer.as_ref().unwrap();
                assert_eq!(
                    (
                        actual.input_revision.0,
                        actual.current,
                        actual.complete,
                        actual.visited_block
                    ),
                    (
                        transfer.input_revision,
                        transfer.current,
                        transfer.complete,
                        transfer.visited_block
                    )
                );
                assert_eq!(actual.calls, transfer.calls);
                assert_eq!(
                    serde_json::to_value(&actual.outcomes).unwrap(),
                    serde_json::to_value(&transfer.outcomes).unwrap()
                );
                assert_eq!(actual.successors.len(), transfer.successors.len());
                for (edge, mapped) in transfer.successors.iter().zip(&actual.successors) {
                    assert_eq!(
                        (mapped.to, mapped.kind),
                        (edge.to, mapping::edge(edge.kind))
                    );
                }
            } else {
                assert!(mapped.transfer.is_none());
            }
        }
        for (call, actual) in source.regional_cfg.calls.iter().zip(&cfg.calls) {
            assert_eq!(
                (
                    actual.id,
                    actual.from,
                    actual.callee,
                    actual.input_revision.0
                ),
                (call.id, call.from, call.callee, call.input_revision)
            );
            assert_eq!(actual.entries, call.entries);
            assert_eq!(actual.deliveries.len(), call.deliveries.len());
            for (delivery, mapped) in call.deliveries.iter().zip(&actual.deliveries) {
                assert_eq!(
                    mapped.kind,
                    match delivery.kind {
                        analysis::OutcomeKind::Return => api::OutcomeKind::Return,
                        analysis::OutcomeKind::Revert => api::OutcomeKind::Revert,
                        analysis::OutcomeKind::Failure => api::OutcomeKind::Failure,
                    }
                );
                assert_eq!(
                    (
                        mapped.input_revision.0,
                        mapped.output_generation,
                        mapped.current
                    ),
                    (
                        delivery.input_revision,
                        delivery.output_generation,
                        delivery.current
                    )
                );
                assert_eq!(mapped.successors.len(), delivery.successors.len());
                for (edge, mapped) in delivery.successors.iter().zip(&mapped.successors) {
                    assert_eq!(
                        (mapped.to, mapped.kind),
                        (edge.to, mapping::edge(edge.kind))
                    );
                }
            }
        }
    }
}

#[test]
fn regional_known_calls_preserve_native_owner_mode_deliveries_and_pooled_roundtrip() {
    let (native, report) = calls_fixture();
    assert!(!report.abstract_summaries.is_empty());
    assert_mapping(&native, &report);
    let nodes: Vec<_> = report
        .abstract_summaries
        .iter()
        .flat_map(|summary| &summary.regional_cfg.nodes)
        .collect();
    for (owner, is_static) in [(0x102, false), (0x103, false), (0x103, true)] {
        assert!(
            nodes
                .iter()
                .any(|node| node.frame.code_address == address(0x103).to_string()
                    && node.frame.storage_address == address(owner).to_string()
                    && node.frame.is_static == is_static)
        );
    }
    assert!(
        report
            .abstract_summaries
            .iter()
            .flat_map(|summary| &summary.regional_cfg.calls)
            .flat_map(|call| &call.deliveries)
            .any(|delivery| delivery.current && !delivery.successors.is_empty())
    );
    let encoded = serde_json::to_vec(&api::AnalyzeReply {
        result: Ok(report.clone()),
    })
    .unwrap();
    let decoded: api::AnalyzeReply = serde_json::from_slice(&encoded).unwrap();
    let decoded = decoded.result.unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, report);
    let raw = serde_json::to_value(render::world::evidence(&native).unwrap()).unwrap();
    assert_eq!(raw["schema_version"], 14);
    assert_eq!(raw["procedure_summaries"][0]["kind"], "KnownCall");
    assert_eq!(
        raw["procedure_summaries"][0]["regional_cfg"],
        serde_json::to_value(&native.procedure_summaries()[0].regional_cfg).unwrap()
    );
    let text = render::world::text(
        &native,
        &ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default())),
    );
    assert!(
        text.contains("KnownCall")
            && text.contains("Regional control flow")
            && text.contains("current delivery")
    );
    assert!(!text.contains("KnownRecursion"));
}

#[test]
fn regional_dynamic_jump_and_both_branch_alternatives_survive_projection() {
    // SLOAD is unknown; the same real dispatcher as the core regression keeps
    // targets 26/49 and excludes the legal but unchosen JUMPDEST at 41.
    let code = "610000541561001057610031610018565b61001a610018565b565b61000b61000052610020610000f35b610000610000fd5b61001661000052610020610000f3";
    let (native, report) = fixture(&[(0x101, ROOT, false), (0x102, code, true)]);
    assert_mapping(&native, &report);
    let program = evm_abstract::bytecode::Program::from_hex_with_fork(code, Fork::Osaka).unwrap();
    let mut targets = std::collections::BTreeSet::new();
    let mut branches = std::collections::BTreeSet::new();
    for summary in &report.abstract_summaries {
        let cfg = &summary.regional_cfg;
        for node in &cfg.nodes {
            let Some(transfer) = &node.transfer else {
                continue;
            };
            for edge in &transfer.successors {
                if edge.kind == api::EdgeKind::BranchTrue {
                    branches.insert(true);
                }
                if edge.kind == api::EdgeKind::BranchFalse {
                    branches.insert(false);
                }
                if node
                    .frame
                    .basic_block
                    .is_some_and(|block| program.blocks()[block].start_pc == 24)
                {
                    assert_eq!(edge.kind, api::EdgeKind::Jump);
                    targets.insert(
                        program.blocks()[cfg.nodes[edge.to].frame.basic_block.unwrap()].start_pc,
                    );
                }
            }
        }
    }
    assert_eq!(targets, std::collections::BTreeSet::from([26, 49]));
    assert_eq!(branches, std::collections::BTreeSet::from([false, true]));
}

#[test]
fn regional_revert_and_exception_deliveries_keep_their_real_caller_targets() {
    for (code, kind, edge_kind) in [
        ("5f5ffd", api::OutcomeKind::Revert, api::EdgeKind::Revert),
        ("fe", api::OutcomeKind::Failure, api::EdgeKind::Failure),
    ] {
        let (native, report) = fixture(&[
            (0x101, ROOT, false),
            (0x102, "5f5f5f5f5f6101035af15000", false),
            (0x103, code, false),
        ]);
        assert_mapping(&native, &report);
        let mut delivered = 0;
        for summary in &report.abstract_summaries {
            let cfg = &summary.regional_cfg;
            for call in &cfg.calls {
                let caller = &cfg.nodes[call.from];
                for delivery in &call.deliveries {
                    if delivery.kind != kind {
                        continue;
                    }
                    assert!(delivery.current);
                    for edge in &delivery.successors {
                        delivered += 1;
                        assert_eq!(edge.kind, edge_kind);
                        assert_eq!(cfg.nodes[edge.to].procedure, caller.procedure);
                    }
                }
            }
        }
        assert!(delivered > 0, "missing real {kind:?} delivery");
    }
}

#[test]
fn regional_invalid_delegation_retains_identity_without_a_program_block() {
    let first = format!("ef0100{}", alloy_primitives::hex::encode(address(0x104)));
    let second = format!("ef0100{}", alloy_primitives::hex::encode(address(0x105)));
    let (native, report) = fixture(&[
        (0x101, ROOT, false),
        (0x102, "5f5f5f5f5f6101035af15000", false),
        (0x103, &first, false),
        (0x104, &second, false),
        (0x105, "00", false),
    ]);
    assert_mapping(&native, &report);
    let raw_hash =
        alloy_primitives::keccak256(alloy_primitives::hex::decode(&second).unwrap()).to_string();
    let text = render::world::text(
        &native,
        &ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default())),
    );
    let mut invalid_nodes = 0;
    for (index, summary) in report.abstract_summaries.iter().enumerate() {
        for node in &summary.regional_cfg.nodes {
            if node.frame.kind != api::CodeKind::InvalidDelegation {
                continue;
            }
            invalid_nodes += 1;
            assert_eq!(node.frame.code_address, address(0x104).to_string());
            assert_eq!(node.frame.code_hash.as_deref(), Some(raw_hash.as_str()));
            assert_eq!(node.frame.storage_address, address(0x103).to_string());
            assert_eq!(
                node.frame.address_value,
                api::AddressValue::Concrete(address(0x103).to_string())
            );
            assert_eq!(
                node.frame.caller,
                api::AddressValue::Concrete(address(0x102).to_string())
            );
            assert!(!node.frame.is_static);
            assert!(node.frame.basic_block.is_none());
            let receipt = node.transfer.as_ref().unwrap();
            assert!(receipt.current && receipt.complete && !receipt.visited_block);
            assert!(receipt.outcomes.contains(&api::OutcomeKind::Failure));

            let relation = text.split_once(&format!("  relation {index} |")).unwrap().1;
            let detail = relation
                .split_once(&format!("      r{} |", node.id))
                .unwrap()
                .1
                .split("\n      r")
                .next()
                .unwrap()
                .split("\n      call ")
                .next()
                .unwrap()
                .split("\n  relation ")
                .next()
                .unwrap();
            assert!(detail.contains("InvalidDelegation") && detail.contains(&raw_hash));
            assert!(
                !detail.contains("program block=")
                    && !detail.contains("pc=")
                    && !detail.contains("SSA")
            );

            let mut fake_block = report.clone();
            fake_block.abstract_summaries[index].regional_cfg.nodes[node.id]
                .frame
                .basic_block = Some(0);
            assert_eq!(
                fake_block.validate(),
                Err(api::ReportValidationFailure::Regional(
                    api::RegionalValidationFailure {
                        summary: index,
                        id: node.id,
                        call: false,
                        kind: api::RegionalValidationKind::BlockVisit,
                    }
                ))
            );
            let mut fake_visit = report.clone();
            fake_visit.abstract_summaries[index].regional_cfg.nodes[node.id]
                .transfer
                .as_mut()
                .unwrap()
                .visited_block = true;
            assert!(matches!(
                fake_visit.validate(),
                Err(api::ReportValidationFailure::Regional(
                    api::RegionalValidationFailure {
                        kind: api::RegionalValidationKind::BlockVisit,
                        ..
                    }
                ))
            ));
        }
    }
    assert!(
        invalid_nodes > 0,
        "the real nested delegation child must enter the regional table"
    );
    let roundtrip: api::AnalysisReport =
        serde_json::from_slice(&serde_json::to_vec(&report).unwrap()).unwrap();
    roundtrip.validate().unwrap();
    assert_eq!(roundtrip.abstract_summaries, report.abstract_summaries);
}

#[test]
fn regional_record_local_references_and_revision_claims_are_validated() {
    let (_, report) = calls_fixture();
    for case in 0..16 {
        let mut bad = report.clone();
        let summary = &mut bad.abstract_summaries[0];
        let cfg = &mut summary.regional_cfg;
        let node_count = cfg.nodes.len();
        let call_count = cfg.calls.len();
        let active = cfg
            .nodes
            .iter()
            .position(|node| node.transfer.is_some())
            .unwrap();
        match case {
            0 => cfg.nodes[0].id = node_count,
            1 => cfg.nodes[0].procedure = summary.procedures.len(),
            2 => cfg.nodes[0].successors.push(node_count),
            3 => cfg.nodes[active]
                .transfer
                .as_mut()
                .unwrap()
                .successors
                .push(api::RegionalEdge {
                    to: node_count,
                    kind: api::EdgeKind::Jump,
                }),
            4 => cfg.nodes[active]
                .transfer
                .as_mut()
                .unwrap()
                .calls
                .push(call_count),
            5 => cfg.calls[0].id = call_count,
            6 => cfg.calls[0].from = node_count,
            7 => cfg.calls[0].callee = summary.procedures.len(),
            8 => cfg.calls[0].entries.push(node_count),
            9 => {
                cfg.nodes[active].input_revision.0 += 1;
                cfg.nodes[active].transfer.as_mut().unwrap().current = true;
            }
            10 => {
                cfg.nodes[active].excluded = true;
                cfg.nodes[active].transfer.as_mut().unwrap().current = true;
            }
            11 => {
                cfg.calls[0].deliveries[0]
                    .successors
                    .push(api::RegionalEdge {
                        to: node_count,
                        kind: api::EdgeKind::SummaryReturn,
                    });
            }
            12 => {
                cfg.calls[0].input_revision.0 += 1;
                cfg.calls[0].deliveries[0].current = true;
            }
            13 => {
                let other = cfg
                    .nodes
                    .iter()
                    .position(|node| node.id != cfg.calls[0].from && node.transfer.is_some())
                    .unwrap();
                cfg.nodes[other].transfer.as_mut().unwrap().calls.push(0);
            }
            14 => {
                let other = cfg
                    .nodes
                    .iter()
                    .position(|node| node.procedure != cfg.calls[0].callee)
                    .unwrap();
                cfg.calls[0].entries.push(other);
            }
            15 => {
                cfg.nodes[active].frame.kind = api::CodeKind::AbstractEnvironment;
                cfg.nodes[active].frame.basic_block = None;
                cfg.nodes[active].transfer.as_mut().unwrap().visited_block = true;
            }
            _ => unreachable!(),
        }
        assert!(
            bad.validate().is_err(),
            "accepted malformed regional case {case}"
        );
    }
    let mut historical = report;
    for summary in &mut historical.abstract_summaries {
        for node in &mut summary.regional_cfg.nodes {
            node.input_revision.0 += 1;
            if let Some(transfer) = &mut node.transfer {
                transfer.current = false;
            }
        }
        for call in &mut summary.regional_cfg.calls {
            call.input_revision.0 += 1;
            for delivery in &mut call.deliveries {
                delivery.current = false;
            }
        }
    }
    historical.validate().unwrap();
}
