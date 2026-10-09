//! World-to-report integration beyond the bytecode-only public request fixtures.
use super::*;
use evm_abstract::{
    Fork,
    world::{AddressInput, EvmEnvironment},
};

#[test]
fn delegated_root_source_uses_code_program_and_retains_authority_storage() {
    let authority = Address::repeat_byte(0x11);
    let target = Address::repeat_byte(0x22);
    let mut world = World::new(Fork::Osaka, "typed delegation report fixture");
    let mut root = Account::empty();
    root.code = Code::Delegation(target);
    world.insert(authority, root).unwrap();
    world
        .insert(target, Account::from_hex("602a00", Fork::Osaka).unwrap())
        .unwrap();
    let graph = analysis::analyze_world(
        world,
        Entry::new(authority),
        analysis::ExecutionConfig::default(),
    )
    .unwrap();
    let report = report::report(
        &graph,
        &api::AnalyzeRequest::default(),
        api::AnalysisScope::RpcWorld,
        &Control::default(),
    )
    .unwrap();
    let source = &report.programs[report.metadata.root_program.unwrap()];
    assert_eq!(source.code_address, target.to_string());
    assert_eq!(source.bytecode, "0x602a00");
    assert_eq!(report.disassembly, source.blocks);
    let frame = &report.states[0].entry.frames[0];
    assert_eq!(frame.code_address, target.to_string());
    assert_eq!(frame.storage_address, authority.to_string());
    assert!(
        matches!(&frame.address_value,api::AddressValue::Concrete(value) if value==&authority.to_string())
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
    let mut world = World::new(Fork::Osaka, "delegatecall fixture");
    world
        .insert(root, Account::from_hex(&code, Fork::Osaka).unwrap())
        .unwrap();
    world
        .insert(
            child,
            Account::from_hex("602a5f5260205ff3", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let environment = EvmEnvironment {
        to: AddressInput::Concrete(root),
        value: evm_abstract::domain::AbstractValue::constant(evm_abstract::U256::ZERO),
        ..EvmEnvironment::default()
    };
    let graph = analysis::analyze_world(
        world,
        Entry {
            address: root,
            environment,
        },
        analysis::ExecutionConfig::default(),
    )
    .unwrap();
    let report = report::report(
        &graph,
        &api::AnalyzeRequest::default(),
        api::AnalysisScope::RpcWorld,
        &Control::default(),
    )
    .unwrap();
    let state = report
        .states
        .iter()
        .find(|state| state.entry.frames.len() == 2)
        .expect("child machine frame");
    let frame = &state.entry.frames[1];
    assert_eq!(frame.storage_address, root.to_string());
    assert_eq!(frame.code_address, child.to_string());
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
            report.byte_arrays[frame.returndata]
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
            AbstractValue,
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
    let projected = value::info(&AbstractValue::top().with_expression(expression));
    let encoded = serde_json::to_vec(&projected).unwrap();
    let decoded: api::ValueInfo = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, projected);
    let graph = decoded.expression.unwrap();
    assert_eq!(graph.root, graph.nodes.len() - 1);
    for (index, node) in graph.nodes.iter().enumerate() {
        if let api::Expression::Operation(operation) = node {
            assert!(operation.arguments.iter().all(|argument| *argument < index));
        }
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
