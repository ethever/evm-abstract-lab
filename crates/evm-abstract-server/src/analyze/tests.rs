//! EvmWorld-to-report integration beyond the bytecode-only public request fixtures.
use super::*;
use evm_abstract::domain::{NumericValue, ProductAlgebra, ValueAlgebra};
use evm_abstract::{
    Fork,
    domain::{DomainSpec, EvmNumericAlgebra},
    world::{AddressInput, EvmEnvironmentValues},
};

/// Fixture execution uses the same canonical borrowed engine as requests.
pub(in crate::analyze) fn execute<D: EvmNumericAlgebra>(
    world: EvmWorld<D::Element>,
    entry: EvmEntry<D::Element>,
    config: analysis::ExecutionConfig,
    algebra: &ValueAlgebra<D>,
) -> analysis::EvmWorldAnalysis<D::Element> {
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let mut engine = AnalysisEngine::new(algebra, &policies, &mut extension);
    engine
        .run(
            AnalysisInput::new(world, entry).unwrap(),
            config,
            &Control::default(),
        )
        .unwrap()
}

fn project_world(world: EvmWorld<NumericValue>, address: Address) -> api::AnalysisReport {
    let config = analysis::ExecutionConfig::default();
    let spec = DomainSpec::default();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(spec));
    let request = api::AnalyzeRequest {
        limits: api::AnalysisLimits {
            max_states: config.analysis.max_states as u64,
            max_transfers: config.analysis.max_transfers as u64,
            context_depth: config.analysis.context_depth as u64,
            max_constants: spec.capacity() as u64,
            max_work: config.max_work as u64,
            max_call_depth: config.max_call_depth as u64,
            max_memory_bytes: config.max_memory_bytes as u64,
            use_summaries: config.use_summaries,
            root_terminal_coverage: config.root_terminal_coverage,
            numeric_model: api::NumericModel::Product,
            reduction_rounds: spec.reduction_rounds() as u64,
            max_facts: spec.fact_limit() as u64,
            relations_enabled: spec.relations().enabled,
            max_constraints: spec.relations().max_constraints as u64,
            max_expression_nodes: spec.relations().max_nodes as u64,
            max_expression_depth: spec.relations().max_depth as u64,
            ..api::AnalysisLimits::default()
        },
        ..api::AnalyzeRequest::default()
    };
    assert_eq!(
        serde_json::to_value(input::config(&request.limits).unwrap()).unwrap(),
        serde_json::to_value(&config).unwrap(),
        "native world fixture execution budgets must match metadata"
    );
    assert_eq!(input::spec(&request.limits).unwrap(), spec);
    let graph = execute(
        world,
        EvmEntry {
            address,
            environment: EvmEnvironmentValues {
                to: AddressInput::Concrete(address),
                value: evm_abstract::domain::EvmValue::<NumericValue>::constant(
                    evm_abstract::U256::ZERO,
                ),
                ..EvmEnvironmentValues::default()
            },
        },
        config,
        &algebra,
    );
    report::report(
        &graph,
        &request,
        api::AnalysisScope::RpcWorld,
        &algebra,
        api::NumericModel::Product,
        &Control::default(),
    )
    .unwrap()
}

#[test]
fn invalid_opcodes_at_the_same_pc_use_each_childs_captured_program() {
    let root = Address::repeat_byte(0x11);
    let first = Address::repeat_byte(0x22);
    let second = Address::repeat_byte(0x33);
    let call = |target: Address| {
        format!(
            "5f5f5f5f5f73{}5af150",
            alloy_primitives::hex::encode(target)
        )
    };
    let code = format!("{}{}00", call(first), call(second));
    let mut world =
        EvmWorld::<NumericValue>::new(Fork::Osaka, "invalid opcode child source fixture");
    world
        .insert(
            root,
            EvmAccount::<NumericValue>::from_hex(&code, Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
        .insert(
            first,
            EvmAccount::<NumericValue>::from_hex("fe", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
        .insert(
            second,
            EvmAccount::<NumericValue>::from_hex("ef", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let report = project_world(world, root);
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    let mut captured = Vec::new();
    for diagnostic in &report.diagnostics {
        let Some(api::InvalidOpcodeDiagnostic::Instruction(instruction)) =
            &diagnostic.invalid_opcode
        else {
            continue;
        };
        assert_eq!(diagnostic.pc, 0);
        let frame = report.states[diagnostic.state].entry.frames.last().unwrap();
        assert_ne!(frame.code_address, Some(root.to_string()));
        let source = &report.programs[frame.program.unwrap()];
        assert_eq!(source.code_address, frame.code_address);
        let bytes = alloy_primitives::hex::decode(&source.bytecode).unwrap();
        assert_eq!(instruction.opcode, bytes[diagnostic.pc]);
        captured.push((frame.code_address.clone(), instruction.opcode));
    }
    captured.sort();
    assert_eq!(
        captured,
        vec![
            (Some(first.to_string()), 0xfe),
            (Some(second.to_string()), 0xef)
        ]
    );
}

#[test]
fn invalid_initcode_opcode_uses_creation_program_at_pc_zero() {
    let creator = Address::repeat_byte(0x11);
    let mut world =
        EvmWorld::<NumericValue>::new(Fork::Osaka, "invalid opcode initcode source fixture");
    // Write FE into memory, then execute that byte as a separate CREATE frame.
    world
        .insert(
            creator,
            EvmAccount::<NumericValue>::from_hex("60fe5f5360015f5ff000", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
        .insert(creator.create(0), EvmAccount::<NumericValue>::absent())
        .unwrap();
    let report = project_world(world, creator);
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.kind == api::DiagnosticKind::InvalidOpcode)
        .unwrap();
    assert_eq!(diagnostic.pc, 0);
    assert_eq!(
        diagnostic.invalid_opcode,
        Some(api::InvalidOpcodeDiagnostic::Instruction(
            api::InvalidInstruction {
                opcode: 0xfe,
                mnemonic: Some("INVALID".into()),
                reason: api::InvalidOpcodeReason::ExplicitInvalid,
            }
        ))
    );
    let frame = report.states[diagnostic.state].entry.frames.last().unwrap();
    let source = &report.programs[frame.program.unwrap()];
    assert_eq!(frame.kind, api::CodeKind::Initcode);
    assert_eq!(source.bytecode, "0xfe");
    assert_ne!(Some(source.id), report.metadata.root_program);
}

#[test]
fn nested_delegation_has_captured_entry_byte_without_instruction_receipt() {
    let authority = Address::repeat_byte(0x11);
    let target = Address::repeat_byte(0x22);
    let implementation = Address::repeat_byte(0x33);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "invalid nested delegation fixture");
    let mut account = EvmAccount::<NumericValue>::empty();
    account.code = Code::Delegation(target);
    world.insert(authority, account).unwrap();
    let mut account = EvmAccount::<NumericValue>::empty();
    account.code = Code::Delegation(implementation);
    world.insert(target, account).unwrap();
    world
        .insert(
            implementation,
            EvmAccount::<NumericValue>::from_hex("fe", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let report = project_world(world, authority);
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.kind, api::DiagnosticKind::InvalidOpcode);
    let Some(api::InvalidOpcodeDiagnostic::DelegationEntry(entry)) = &diagnostic.invalid_opcode
    else {
        panic!("delegation entry must keep its distinct source");
    };
    assert_eq!(entry.opcode, Some(0xef));
    assert_eq!(entry.byte_unavailable, None);
    assert!(entry.detail.contains("nested delegation"));
    let state = &report.states[diagnostic.state];
    assert!(state.entry.frames.last().unwrap().program.is_none());
    let cfg = &report.cfg[diagnostic.state];
    assert!(cfg.instructions.is_empty());
    assert!(cfg.executed_pcs.is_empty());
    assert!(report.ssa.blocks[diagnostic.state].instructions.is_empty());
    assert_eq!(
        serde_json::from_slice::<api::AnalysisReport>(&serde_json::to_vec(&report).unwrap())
            .unwrap(),
        report
    );
}

#[test]
fn delegation_projection_retains_why_original_byte_is_unavailable() {
    let reason = analysis::DiagnosticKind::InvalidOpcode(
        analysis::InvalidOpcodeDiagnostic::DelegationEntry {
            opcode: None,
            detail: "nested delegation indicator cannot be executed".into(),
            byte_unavailable: Some("active code account's raw bytes are unavailable".into()),
        },
    );
    assert_eq!(
        mapping::invalid_opcode(&reason),
        Some(api::InvalidOpcodeDiagnostic::DelegationEntry(
            api::InvalidDelegationEntry {
                opcode: None,
                detail: "nested delegation indicator cannot be executed".into(),
                byte_unavailable: Some("active code account's raw bytes are unavailable".into()),
            }
        ))
    );
}

#[test]
fn delegated_root_source_uses_code_program_and_retains_authority_storage() {
    let authority = Address::repeat_byte(0x11);
    let target = Address::repeat_byte(0x22);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "typed delegation report fixture");
    let mut root = EvmAccount::<NumericValue>::empty();
    root.code = Code::Delegation(target);
    world.insert(authority, root).unwrap();
    world
        .insert(
            target,
            EvmAccount::<NumericValue>::from_hex("602a00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let graph = execute(
        world,
        EvmEntry::<NumericValue>::new(authority),
        analysis::ExecutionConfig::default(),
        &ValueAlgebra::new(ProductAlgebra::default()),
    );
    let report = report::report(
        &graph,
        &api::AnalyzeRequest::default(),
        api::AnalysisScope::RpcWorld,
        &ValueAlgebra::new(ProductAlgebra::from_spec(graph.domain_spec())),
        api::NumericModel::Product,
        &Control::default(),
    )
    .unwrap();
    let source = &report.programs[report.metadata.root_program.unwrap()];
    assert_eq!(source.code_address, Some(target.to_string()));
    assert_eq!(source.bytecode, "0x602a00");
    assert_eq!(report.disassembly, source.blocks);
    let frame = &report.states[0].entry.frames[0];
    assert_eq!(frame.code_address, Some(target.to_string()));
    assert_eq!(frame.storage_address, Some(authority.to_string()));
    assert!(
        matches!(&frame.address_value,Some(api::AddressValue::Concrete(value)) if value==&authority.to_string())
    );
    let authority = report
        .accounts
        .iter()
        .find(|item| item.address == authority.to_string())
        .unwrap();
    assert_eq!(authority.code_kind, api::CodeKind::Delegation);
    assert_eq!(authority.delegation_target, Some(target.to_string()));
}
#[test]
fn child_frames_reference_complete_programs_private_memory_and_parent_continuation() {
    let root = Address::repeat_byte(0x11);
    let child = Address::repeat_byte(0x22);
    let code = format!("60205f5f5f73{}5af400", alloy_primitives::hex::encode(child));
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "delegatecall fixture");
    world
        .insert(
            root,
            EvmAccount::<NumericValue>::from_hex(&code, Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
        .insert(
            child,
            EvmAccount::<NumericValue>::from_hex("602a5f5260205ff3", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let environment = EvmEnvironmentValues {
        to: AddressInput::Concrete(root),
        value: evm_abstract::domain::EvmValue::<NumericValue>::constant(evm_abstract::U256::ZERO),
        ..EvmEnvironmentValues::<NumericValue>::default()
    };
    let graph = execute(
        world,
        EvmEntry {
            address: root,
            environment,
        },
        analysis::ExecutionConfig::default(),
        &ValueAlgebra::new(ProductAlgebra::default()),
    );
    let report = report::report(
        &graph,
        &api::AnalyzeRequest::default(),
        api::AnalysisScope::RpcWorld,
        &ValueAlgebra::new(ProductAlgebra::from_spec(graph.domain_spec())),
        api::NumericModel::Product,
        &Control::default(),
    )
    .unwrap();
    let state = report
        .states
        .iter()
        .find(|state| state.entry.frames.len() == 2)
        .expect("child machine frame");
    let frame = &state.entry.frames[1];
    assert_eq!(frame.storage_address, Some(root.to_string()));
    assert_eq!(frame.code_address, Some(child.to_string()));
    assert!(frame.continuation.is_some());
    let source = &report.programs[frame.program.unwrap()];
    assert_eq!(source.bytecode, "0x602a5f5260205ff3");
    assert_eq!(source.blocks[0].instructions.last().unwrap().name, "RETURN");
    assert_eq!(
        report
            .cfg
            .iter()
            .find(|cfg| cfg.id == state.state)
            .unwrap()
            .program,
        frame.program
    );
    assert!(
        report
            .ssa
            .blocks
            .iter()
            .any(|block| block.state == state.state)
    );
    let returned = report
        .states
        .iter()
        .filter_map(|state| state.exit.as_ref())
        .flat_map(|machine| &machine.frames)
        .any(|frame| {
            report.byte_arrays[frame.returndata.unwrap()]
                .length
                .constants
                .as_ref()
                .is_some_and(|lengths| lengths.contains(&"0x20".to_string()))
        });
    assert!(returned, "caller keeps the complete child returndata");
}

mod rpc;

#[test]
fn deepest_admitted_expression_has_a_flat_roundtrippable_typed_graph() {
    use evm_abstract::{
        U256,
        domain::{
            EvmValue,
            identity::Symbol,
            symbolic::{ExprId, ExprLimits},
        },
    };
    let mut expression = ExprId::input(77, Symbol::CalldataWord(U256::ZERO));
    let limits = ExprLimits {
        max_nodes: 512,
        max_depth: 128,
    };
    for _ in 1..128 {
        expression =
            ExprId::operation(0x01, &[expression, ExprId::constant(U256::ONE)], limits).unwrap();
    }
    assert_eq!(expression.depth(), 128);
    let control = Control::default();
    let mut pool = super::symbolic::Pool::new(&control);
    let projected = pool
        .value(&EvmValue::<NumericValue>::top().with_expression(expression))
        .unwrap();
    let encoded = serde_json::to_vec(&(&projected, &pool.graph)).unwrap();
    assert!(
        encoded.len() < 40_000,
        "a shared depth-128 graph must not recursively expand"
    );
    let (decoded, graph): (api::ValueInfo, api::SymbolicGraph) =
        serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, projected);
    assert_eq!(graph, pool.graph);
    let root = decoded.expression.unwrap();
    assert_eq!(root.node, pool.graph.nodes.len() - 1);
    pool.graph.validate().unwrap();
    for (index, node) in pool.graph.nodes.iter().enumerate() {
        assert!(node.references().iter().all(|child| child.node < index));
    }
}

#[test]
fn rpc_nested_failure_keeps_native_hashes_and_coordinates_in_json() {
    use alloy_primitives::B256;
    use evm_abstract::{
        U256,
        world::{WorldError, rpc as native},
    };
    let address = Address::repeat_byte(0x22);
    let expected = B256::repeat_byte(0x11);
    let observed = B256::repeat_byte(0x33);
    let source = native::RpcError::World {
        context: Box::new(native::RpcContext {
            chain_id: Some(U256::ONE),
            block_hash: Some(B256::repeat_byte(0x44)),
            method: "eth_getCode",
            account: Some(address),
            slot: None,
        }),
        source: WorldError::CodeHash {
            address,
            expected,
            observed,
        },
    };
    let error = super::rpc::error(source);
    let bytes = serde_json::to_vec(&error).unwrap();
    let decoded: api::ApiError = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error, decoded);
    let api::ErrorDetails::Rpc(failure) = decoded.details else {
        panic!("RPC error detail")
    };
    assert_eq!(failure.account, Some(address.to_string()));
    assert_eq!(failure.chain_id.as_deref(), Some("0x1"));
    let Some(api::RpcFailureCause::World(api::WorldFailure::CodeHash(cause))) = failure.cause
    else {
        panic!("typed code hash cause")
    };
    assert_eq!(cause.address, address.to_string());
    assert_eq!(cause.expected, expected.to_string());
    assert_eq!(cause.observed, observed.to_string());
}

#[test]
fn relational_frontier_projection_preserves_all_local_causes() {
    use evm_abstract::domain::{
        facts::{FactError, Symbol},
        relational::QueryReason,
    };
    let cases = [
        (
            QueryReason::ScalarFactError(FactError::InvalidOperation {
                opcode: 0x16,
                expected: 2,
                actual: 1,
            }),
            api::QueryBoundary::ScalarFactError(api::FactFailure::InvalidOperation(
                api::FactOperationFailure {
                    opcode: 0x16,
                    expected: 2,
                    actual: 1,
                },
            )),
        ),
        (
            QueryReason::ScalarFactError(FactError::Contradiction {
                subject: Symbol::new(17),
            }),
            api::QueryBoundary::ScalarFactError(api::FactFailure::Contradiction(17)),
        ),
        (QueryReason::Cancelled, api::QueryBoundary::Cancelled),
        (QueryReason::Disabled, api::QueryBoundary::Disabled),
        (
            QueryReason::Configuration,
            api::QueryBoundary::Configuration,
        ),
        (
            QueryReason::ConstraintLimit,
            api::QueryBoundary::ConstraintLimit,
        ),
        (
            QueryReason::ScalarFactLimit,
            api::QueryBoundary::ScalarFactLimit,
        ),
        (
            QueryReason::ExpressionLimit,
            api::QueryBoundary::ExpressionLimit,
        ),
    ];
    for (native, expected) in cases {
        let projected = mapping::reason(&analysis::FrontierReason::Relations(native));
        assert_eq!(projected, api::FrontierDetails::Relations(expected));
        let encoded = serde_json::to_vec(&projected).unwrap();
        assert_eq!(
            serde_json::from_slice::<api::FrontierDetails>(&encoded).unwrap(),
            projected
        );
    }
}

mod known_recursion;
mod numeric;
mod regional;
mod symbolic;
mod unknown;
