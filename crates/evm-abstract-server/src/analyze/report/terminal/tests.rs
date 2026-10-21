//! Source regressions for real native attachment and the full wire boundary.
use super::*;
use crate::analyze::{input, report};
use alloy_primitives::{Address, B256, U256};
use evm_abstract::{
    analysis::{WorldAnalysis, control::Control},
    domain::{AbstractValue, Domain},
    world::{Account, Entry, Store, World},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

// Reuse the core's two-block cap fixture through public execution APIs. The
// transfer cap interrupts actual detail; no certificate is minted by the test.
const TWO_BLOCK_RETURN: &str = "6003565b602a5f5260205ff3";

fn fixture(enabled: bool, is_static: bool, chain: bool) -> (WorldAnalysis, api::AnalyzeRequest) {
    let owner = Address::repeat_byte(0x11);
    let other = Address::repeat_byte(0x22);
    let mut request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: TWO_BLOCK_RETURN.into(),
            address: None,
        }),
        ..api::AnalyzeRequest::default()
    };
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
    // Storage does not decide EIP-161 emptiness. This MAY-empty owner makes
    // future sticky address-3 cleanup retain an observable delete/keep root.
    let mut ripemd = Account::unknown();
    ripemd
        .storage
        .insert(U256::from(7), AbstractValue::constant(U256::from(7)));
    world
        .insert(Address::from_word(U256::from(3).into()), ripemd)
        .unwrap();
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

// A single traversal names the entire joint tuple, including original sources.
// Only directory node IDs and producer Fresh labels are renamed. Scoped inputs,
// source namespaces, every DTO field and all cross-field sharing stay exact.
fn canonical_joint(mut tuple: Value, graph: &api::SymbolicGraph) -> Value {
    fn visit(
        value: &mut Value,
        graph: &api::SymbolicGraph,
        ids: &mut BTreeMap<usize, usize>,
        fresh: &mut BTreeMap<u64, u64>,
        nodes: &mut Vec<Value>,
    ) {
        if value
            .as_object()
            .is_some_and(|v| v.len() == 1 && v.contains_key("node"))
        {
            let original = usize::try_from(value["node"].as_u64().unwrap()).unwrap();
            let next = ids.len();
            let id = *ids.entry(original).or_insert(next);
            if id == next {
                nodes.push(Value::Null);
                let mut node = match &graph.nodes[original] {
                    api::Expression::Fresh(label) => {
                        let next = fresh.len() as u64;
                        json!(api::Expression::Fresh(*fresh.entry(*label).or_insert(next)))
                    }
                    node => serde_json::to_value(node).unwrap(),
                };
                visit(&mut node, graph, ids, fresh, nodes);
                nodes[id] = node;
            }
            value["node"] = json!(id);
        } else {
            match value {
                Value::Array(values) => {
                    for value in values {
                        visit(value, graph, ids, fresh, nodes);
                    }
                }
                Value::Object(values) => {
                    for value in values.values_mut() {
                        visit(value, graph, ids, fresh, nodes);
                    }
                }
                _ => {}
            }
        }
    }
    graph.validate().unwrap();
    let mut nodes = Vec::new();
    visit(
        &mut tuple,
        graph,
        &mut BTreeMap::new(),
        &mut BTreeMap::new(),
        &mut nodes,
    );
    json!([tuple, nodes])
}

fn report_joint(report: &api::AnalysisReport) -> Value {
    let coverage = report.root_terminal_coverage.as_ref().unwrap();
    let input = &coverage.input;
    let mut environment = serde_json::to_value(&input.environment).unwrap();
    environment["calldata"] = json!(report.byte_arrays[input.environment.calldata]);
    let outcomes: Vec<_> = coverage
        .outcomes
        .iter()
        .map(|outcome| {
            (
                outcome.kind,
                &report.stores[outcome.store],
                &report.byte_arrays[outcome.data],
            )
        })
        .collect();
    json!([
        input.storage_source,
        input.input_scope,
        report.byte_arrays[input.calldata],
        input.call_value,
        report.stores[input.saved_store],
        report.stores[input.initial_store],
        outcomes,
        report.programs,
        input.frame.is_static,
        environment
    ])
}

fn native_joint(source: &analysis::RootTerminalCoverage, pools: &mut Pools<'_>) -> Value {
    // Deliberately use a different directory order from report/project. This
    // oracle selects each actual native leaf, without reusing terminal::project.
    let mut outcomes: Vec<_> = source
        .outcomes()
        .iter()
        .rev()
        .map(|outcome| {
            let store = pools.store(outcome.store()).unwrap();
            let data = pools.buffer(outcome.data()).unwrap();
            (
                outcome.kind(),
                pools.stores[store].clone(),
                pools.buffers_dto[data].clone(),
            )
        })
        .collect();
    outcomes.reverse();
    let input = source.input();
    let initial = pools.store(input.initial_store()).unwrap();
    let saved = pools.store(input.saved_store().state()).unwrap();
    let calldata = pools.buffer(input.calldata()).unwrap();
    let call_value = pools.symbolic.value(input.call_value()).unwrap();
    let storage_source = pools.symbolic.expression(input.storage_source()).unwrap();
    let environment = environment::project(&input.entry().environment, pools).unwrap();
    let mut original_environment = serde_json::to_value(&environment).unwrap();
    original_environment["calldata"] = json!(pools.buffers_dto[environment.calldata]);
    json!([
        storage_source,
        input.input_scope().map(api::DecimalU64),
        pools.buffers_dto[calldata],
        call_value,
        pools.stores[saved],
        pools.stores[initial],
        outcomes,
        pools.programs,
        input.frame().is_static,
        original_environment
    ])
}

#[test]
fn explicit_opt_out_and_default_converged_none_preserve_honest_detail() {
    assert!(api::AnalysisLimits::default().root_terminal_coverage);
    assert!(
        input::config(&api::AnalysisLimits::default())
            .unwrap()
            .root_terminal_coverage
    );
    let (ordinary, request) = fixture(false, false, false);
    let ordinary = project(&ordinary, &request);
    assert_eq!(ordinary.status, api::AnalysisStatus::Incomplete);
    assert!(!ordinary.metadata.limits.root_terminal_coverage);
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
    let request = api::AnalyzeRequest::default();
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
    assert_eq!(report.schema_version, 16);
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
    assert_eq!(binding.domain_spec.schema_version, 2);
    assert_eq!(binding.domain_spec.word_bits, domain.word_bits());
    assert_eq!(binding.domain_spec.cost_version, domain.cost_version());
    assert_eq!(binding.domain_spec.cost_version, 6);
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
    // Rollback alternatives finalize independently: equal sets need not share
    // cleanup selectors. Match each full Store+data to its own native outcome.
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let expected = native_joint(source, &mut pools);
    let actual = canonical_joint(report_joint(&report), &report.symbolic);
    assert_eq!(
        actual,
        canonical_joint(expected.clone(), &pools.symbolic.graph)
    );

    let ripemd = Address::from_word(U256::from(3).into()).to_string();
    let account = |store: usize| {
        report.stores[store]
            .accounts
            .iter()
            .find(|account| account.address == ripemd)
            .unwrap()
    };
    let reverted = account(coverage.outcomes[1].store);
    let failed = account(coverage.outcomes[2].store);
    let api::Expression::SlotsIte(reverted_root) =
        &report.symbolic.nodes[reverted.storage_root.node]
    else {
        panic!("observable address-3 rollback cleanup")
    };
    let api::Expression::SlotsIte(failed_root) = &report.symbolic.nodes[failed.storage_root.node]
    else {
        panic!("observable address-3 failure cleanup")
    };
    assert_ne!(reverted_root.condition, failed_root.condition);
    for which in 0..5 {
        let mut wrong = report.clone();
        match which {
            0 => wrong.stores[coverage.outcomes[2].store]
                .accounts
                .retain(|account| account.address != ripemd),
            1 => {
                let owner = wrong.stores[coverage.outcomes[2].store]
                    .accounts
                    .iter_mut()
                    .find(|account| account.address == ripemd)
                    .unwrap();
                owner
                    .storage
                    .iter_mut()
                    .find(|entry| entry.slot == "0x7")
                    .unwrap()
                    .value = value::info(&AbstractValue::constant(U256::from(8)));
            }
            2 => {
                let api::Expression::SlotsIte(root) =
                    &mut wrong.symbolic.nodes[failed.storage_root.node]
                else {
                    unreachable!()
                };
                root.condition = reverted_root.condition;
            }
            3 => {
                let api::Expression::SlotsInput(root) =
                    &mut wrong.symbolic.nodes[returned.storage_root.node]
                else {
                    unreachable!()
                };
                root.source.token = binding.storage_source;
            }
            _ => {
                let api::Expression::Input(identity) =
                    &mut wrong.symbolic.nodes[binding.call_value.expression.unwrap().node]
                else {
                    unreachable!()
                };
                identity.scope = api::DecimalU64(u64::MAX);
            }
        }
        assert_ne!(
            canonical_joint(report_joint(&wrong), &wrong.symbolic),
            actual
        );
    }
    let mut changed_native_input = source.input().initial_store().clone();
    changed_native_input
        .write_checked(
            Address::from_word(U256::from(3).into()),
            &AbstractValue::constant(U256::from(7)),
            &AbstractValue::constant(U256::from(8)),
            Domain::from_spec(source.input().domain_spec()),
        )
        .unwrap();
    let changed = pools.store(&changed_native_input).unwrap();
    let mut wrong_native = expected;
    wrong_native[5] = serde_json::to_value(&pools.stores[changed]).unwrap();
    assert_ne!(canonical_joint(wrong_native, &pools.symbolic.graph), actual);
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
fn code_only_input_keeps_symbolic_to_and_captured_code_despite_unknown_return_code() {
    let mut request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: TWO_BLOCK_RETURN.into(),
            address: None,
        }),
        ..api::AnalyzeRequest::default()
    };
    request.limits.max_transfers = 1;
    let limits = request.limits.clone();
    assert_eq!(limits.max_work, 2_000_000_000);
    assert!(limits.root_terminal_coverage);
    let encoded = serde_json::to_string(&request).unwrap();
    for (field, value) in [
        ("max_work", "2000000000"),
        ("root_terminal_coverage", "true"),
    ] {
        let missing = encoded.replace(&format!("\"{field}\":{value},"), "");
        assert_ne!(missing, encoded, "field was not removed: {field}");
        let error = serde_json::from_str::<api::AnalyzeRequest>(&missing).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("missing field: {field}")),
            "{error}"
        );
    }
    let request: api::AnalyzeRequest = serde_json::from_str(&encoded).unwrap();
    assert_eq!(request.limits, limits);
    let report = crate::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    assert_eq!(report.status, api::AnalysisStatus::Incomplete);
    assert!(!report.ssa.complete);
    assert!(
        report
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == api::FrontierKind::Transfers)
    );
    assert_eq!(report.metadata.limits, limits);
    let coverage = report.root_terminal_coverage.as_ref().unwrap();
    assert_eq!(coverage.trigger, api::RootTerminalTrigger::Transfers);
    let input = &coverage.input;
    assert_eq!(input.config.max_work, limits.max_work);
    assert_eq!(input.config.max_transfers, limits.max_transfers);
    assert_eq!(
        input.config.root_terminal_coverage,
        limits.root_terminal_coverage
    );
    assert_eq!(
        input.environment.to,
        api::AddressValue::Symbolic(value::symbol(evm_abstract::domain::identity::Symbol::To))
    );
    assert_eq!(input.frame.address_value, input.environment.to);
    assert_eq!(input.frame.address, input.entry_address);
    let program = report
        .programs
        .iter()
        .find(|program| program.id == input.program)
        .unwrap();
    assert_eq!(program.kind, api::CodeKind::Runtime);
    assert_eq!(program.code_hash, input.frame.code_hash);
    assert_eq!(program.bytecode, format!("0x{TWO_BLOCK_RETURN}"));
    let returned = &report.stores[coverage.outcomes[0].store];
    assert!(returned.code_default.is_none());
    assert!(returned.accounts.is_empty());
}

#[test]
fn original_root_frame_requires_the_admitted_runtime_entry_shape() {
    let (native, request) = fixture(true, false, false);
    let report = project(&native, &request);
    let input = &report.root_terminal_coverage.as_ref().unwrap().input;
    let blocks = report
        .programs
        .iter()
        .find(|program| program.id == input.program)
        .unwrap()
        .blocks
        .len();
    assert!(blocks > 1);
    assert_eq!(input.frame.mode, api::RootTerminalFrameCode::Runtime);
    assert_eq!(input.frame.basic_block_index, 0);
    assert_eq!(input.frame.stack_height, 0);
    assert!(input.frame.jump_history.is_empty());
    for (basic_block_index, stack_height, jump_history) in [
        (blocks, 0, vec![]),
        (1, 0, vec![]),
        (0, 1, vec![]),
        (0, 0, vec![0]),
    ] {
        let mut invalid = report.clone();
        let frame = &mut invalid.root_terminal_coverage.as_mut().unwrap().input.frame;
        frame.basic_block_index = basic_block_index;
        frame.stack_height = stack_height;
        frame.jump_history = jump_history;
        assert_eq!(
            invalid.validate(),
            Err(api::ReportValidationFailure::RootTerminal(
                api::RootTerminalValidationFailure::Frame
            ))
        );
    }
    for (mode, kind) in [
        (
            api::RootTerminalFrameCode::InitCode,
            api::CodeKind::Initcode,
        ),
        (api::RootTerminalFrameCode::Empty, api::CodeKind::Empty),
        (
            api::RootTerminalFrameCode::Precompile("0x1".into()),
            api::CodeKind::Precompile,
        ),
        (
            api::RootTerminalFrameCode::InvalidDelegation,
            api::CodeKind::InvalidDelegation,
        ),
        (
            api::RootTerminalFrameCode::Unknown,
            api::CodeKind::AbstractEnvironment,
        ),
    ] {
        let mut invalid = report.clone();
        let input = &mut invalid.root_terminal_coverage.as_mut().unwrap().input;
        input.frame.mode = mode;
        // Matching a non-runtime program kind cannot admit a different root.
        invalid
            .programs
            .iter_mut()
            .find(|program| program.id == input.program)
            .unwrap()
            .kind = kind;
        assert_eq!(
            invalid.validate(),
            Err(api::ReportValidationFailure::RootTerminal(
                api::RootTerminalValidationFailure::Frame
            ))
        );
    }
}

#[test]
fn native_mandatory_retention_site_requires_the_same_exported_frontier() {
    // Disjoint SSTOREs grow the ordered slot root without memory's optional
    // byte reader. The initial JUMP leaves a real state for the mismatch case.
    let mut bytecode = String::from("6003565b");
    for slot in 0..32 {
        bytecode.push_str(&format!("3461{slot:04x}55"));
    }
    bytecode.push_str("00");
    let owner = Address::repeat_byte(0x11);
    let mut world = World::offline(Fork::Osaka, "raw retention site", "wire fixture");
    world
        .insert(owner, Account::from_hex(&bytecode, Fork::Osaka).unwrap())
        .unwrap();
    let mut request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode,
            address: None,
        }),
        ..api::AnalyzeRequest::default()
    };
    request.limits.root_terminal_coverage = true;
    request.limits.max_expression_nodes = 64;
    let entry = Entry {
        address: owner,
        environment: input::environment(&request.environment).unwrap(),
    };
    let config = input::config(&request.limits).unwrap();
    let diagnostic = |native: &WorldAnalysis| {
        format!(
            "status={:?}, frontiers={:?}, work={}, states={}, executed_pcs={:?}",
            native.status(),
            native
                .frontiers()
                .iter()
                .map(|frontier| (frontier.from, frontier.pc, &frontier.reason))
                .collect::<Vec<_>>(),
            native.work(),
            native.states().len(),
            native
                .states()
                .iter()
                .map(|state| (state.id, &state.executed_pcs))
                .collect::<Vec<_>>()
        )
    };
    // The same input and node/work bounds must admit preparation before the
    // write body runs; None alone cannot tell us whether preparation succeeded.
    let mut admission_config = config.clone();
    admission_config.analysis.max_transfers = 1;
    let admitted = analysis::analyze_world(world.clone(), entry.clone(), admission_config).unwrap();
    assert_eq!(
        admitted
            .root_terminal_coverage()
            .map(|source| source.trigger()),
        Some(&analysis::RootTerminalTrigger::Transfers),
        "root preparation control: {}",
        diagnostic(&admitted)
    );
    let native = analysis::analyze_world(world, entry, config).unwrap();
    // Obtain the cause only from the immutable native attachment, never from
    // a flattened frontier category or a manually constructed certificate.
    let source = native
        .root_terminal_coverage()
        .unwrap_or_else(|| panic!("missing mandatory retention: {}", diagnostic(&native)));
    let analysis::RootTerminalTrigger::MandatoryRetention { state, pc, kind } = source.trigger()
    else {
        panic!(
            "expected actual mandatory retention, got {:?}: {}",
            source.trigger(),
            diagnostic(&native)
        )
    };
    assert_eq!(*kind, analysis::RootTerminalRetention::State);
    assert_eq!(source.input().program().bytes().get(*pc), Some(&0x55));
    assert!(native.states()[*state].executed_pcs.contains(pc));
    assert!(native.frontiers().iter().any(|frontier| {
        frontier.from == Some(*state)
            && frontier.pc == Some(*pc)
            && frontier.reason
                == analysis::FrontierReason::Relations(
                    evm_abstract::domain::relational::QueryReason::ExpressionLimit,
                )
    }));
    let report = project(&native, &request);
    assert_eq!(report.status, api::AnalysisStatus::Incomplete);
    let expected = api::RootTerminalRetentionTrigger {
        state: *state,
        pc: *pc,
        kind: match kind {
            analysis::RootTerminalRetention::Expression => api::RootTerminalRetention::Expression,
            analysis::RootTerminalRetention::State => api::RootTerminalRetention::State,
        },
    };
    assert_eq!(
        report.root_terminal_coverage.as_ref().unwrap().trigger,
        api::RootTerminalTrigger::MandatoryRetention(expected.clone())
    );
    let other_state = report
        .states
        .iter()
        .find(|state| state.state != expected.state)
        .unwrap()
        .state;
    for which in 0..3 {
        let mut invalid = report.clone();
        let api::RootTerminalTrigger::MandatoryRetention(trigger) =
            &mut invalid.root_terminal_coverage.as_mut().unwrap().trigger
        else {
            unreachable!()
        };
        match which {
            0 => trigger.pc = usize::MAX,
            1 => invalid.frontiers.retain(|frontier| {
                frontier.from != Some(expected.state) || frontier.pc != Some(expected.pc)
            }),
            _ => trigger.state = other_state,
        }
        assert!(
            invalid
                .states
                .iter()
                .any(|state| state.state == expected.state)
        );
        assert_eq!(
            invalid.validate(),
            Err(api::ReportValidationFailure::RootTerminal(
                api::RootTerminalValidationFailure::RetentionSite
            ))
        );
    }
    let mut invalid = report;
    let api::RootTerminalTrigger::MandatoryRetention(trigger) =
        &mut invalid.root_terminal_coverage.as_mut().unwrap().trigger
    else {
        unreachable!()
    };
    trigger.state = usize::MAX;
    assert!(matches!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Pool(
            api::PoolReferenceFailure {
                kind: api::PoolReferenceKind::MachineState,
                index: usize::MAX,
                ..
            }
        ))
    ));
}

#[test]
fn chain_identity_and_static_nonselected_account_state_survive_projection() {
    let (native, request) = fixture(true, true, true);
    let report = project(&native, &request);
    let coverage = report.root_terminal_coverage.as_ref().unwrap();
    let source = native.root_terminal_coverage().unwrap();
    assert!(source.input().frame().is_static);
    assert!(coverage.input.frame.is_static);
    assert!(coverage.input.environment.is_static);
    assert!(report.metadata.environment.is_static);
    let control = Control::default();
    let mut pools = Pools::new(&control);
    let expected = native_joint(source, &mut pools);
    assert_eq!(
        canonical_joint(report_joint(&report), &report.symbolic),
        canonical_joint(expected, &pools.symbolic.graph)
    );
    let api::RootTerminalSnapshot::Chain(snapshot) = &coverage.input.snapshot else {
        panic!("chain snapshot")
    };
    assert_eq!(snapshot.chain_id, value::word(U256::MAX));
    assert_eq!(snapshot.block_hash, B256::repeat_byte(0x45).to_string());
    let saved = &report.stores[coverage.input.saved_store];
    let initial = &report.stores[coverage.input.initial_store];
    let returned = &report.stores[coverage.outcomes[0].store];
    let reverted = &report.stores[coverage.outcomes[1].store];
    let failed = &report.stores[coverage.outcomes[2].store];
    for store in [saved, initial, reverted, failed] {
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
    for after in [reverted, failed] {
        for address in [Address::repeat_byte(0x11), Address::repeat_byte(0x22)] {
            let account = saved
                .accounts
                .iter()
                .find(|account| account.address == address.to_string())
                .unwrap();
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
    // Static success uses the same independent all-account effects as ordinary
    // success, then real cleanup; original/rollback precision stays above.
    assert!(returned.accounts.is_empty());
    assert!(returned.code_default.is_none());
    assert_eq!(returned.existence_default, api::AccountExistence::Unknown);
    assert_eq!(returned.balance_default, value::info(&AbstractValue::top()));
    assert_eq!(returned.nonce_default, value::info(&AbstractValue::top()));
    assert_eq!(returned.storage_default, value::info(&AbstractValue::top()));
    assert_eq!(
        returned.transient_default,
        value::info(&AbstractValue::constant(U256::ZERO))
    );
    assert_eq!(returned.created_default, Some(false));
    assert_eq!(returned.pending_destruction_default, Some(false));
    assert_eq!(returned.touched_default, Some(false));
    assert!(returned.logs.is_empty());
    assert!(returned.logs_unknown);
    assert_ne!(returned.storage_root, initial.storage_root);
    assert_eq!(returned.transient_root, initial.transient_root);
    assert!(matches!(
        report.symbolic.nodes[returned.storage_root.node],
        api::Expression::SlotsInput(_)
    ));
    assert_eq!(
        report.symbolic.nodes[returned.transient_root.node],
        api::Expression::SlotsZero
    );
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
    invalid.schema_version = 15;
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Schema(15))
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
