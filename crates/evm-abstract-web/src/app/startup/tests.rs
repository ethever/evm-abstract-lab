//! Bootstrap follows the accepted catalog once; subsequent input belongs to the user.

use super::{Command, TaskPhase, Workspace};
use crate::{Message, TransportError};
use evm_abstract_protocol::{AnalysisInput, RpcProvider, RpcProvidersReply};

const USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";

fn providers() -> Vec<RpcProvider> {
    vec![RpcProvider {
        id: "ethereum".into(),
        name: "Ethereum mainnet".into(),
        endpoint: "http://127.0.0.1:8545".into(),
    }]
}

fn catalog(workspace: &mut Workspace, generation: u64, providers: Vec<RpcProvider>) {
    workspace.receive_message(Message::RpcProviders {
        generation,
        result: Ok(RpcProvidersReply {
            result: Ok(providers),
        }),
    });
}

fn assert_usdc(command: Option<Command>) {
    let Some(Command::Submit { request, .. }) = command else {
        panic!("expected an initial analysis submission")
    };
    let AnalysisInput::Rpc(input) = request.input else {
        panic!("configured startup must select an RPC account")
    };
    assert_eq!(input.address, USDC);
    assert_eq!(input.provider_id, "ethereum");
    assert_eq!(input.block, evm_abstract_protocol::BlockSelector::Latest);
    assert_eq!(
        request.environment,
        evm_abstract_protocol::EnvironmentInput::default()
    );
    assert_eq!(
        request.limits,
        evm_abstract_protocol::AnalysisLimits::default()
    );
}

#[test]
fn matching_provider_catalog_starts_usdc_once_without_a_bytecode_submission() {
    let mut workspace = Workspace::default();
    assert!(matches!(
        workspace.startup_command(),
        Some(Command::RpcProviders { generation: 1 })
    ));
    assert_eq!(workspace.task.phase, TaskPhase::Idle);
    assert!(
        workspace
            .accessible_status()
            .contains("Loading the initial example")
    );
    assert!(workspace.startup_command().is_none());
    assert!(workspace.rpc_provider_command().is_none());
    catalog(&mut workspace, 2, providers());
    assert!(workspace.pending_startup_command().is_none());
    catalog(&mut workspace, 1, providers());
    assert_usdc(workspace.pending_startup_command());
    assert_eq!(workspace.task.phase, TaskPhase::Submitting);
    assert!(workspace.pending_startup_command().is_none());
    catalog(&mut workspace, 1, providers());
    assert!(workspace.pending_startup_command().is_none());
}

#[test]
fn an_empty_or_failed_catalog_uses_the_offline_branch_example() {
    for failed in [false, true] {
        let mut workspace = Workspace::default();
        workspace.startup_command();
        if failed {
            workspace.receive_message(Message::RpcProviders {
                generation: 1,
                result: Err(TransportError::Timeout { seconds: 15 }),
            });
        } else {
            catalog(&mut workspace, 1, vec![]);
        }
        let Some(Command::Submit { request, .. }) = workspace.pending_startup_command() else {
            panic!("fallback must submit the offline example")
        };
        let AnalysisInput::Bytecode(input) = request.input else {
            panic!("unavailable RPC must leave the bytecode example usable")
        };
        assert_eq!(input.bytecode, crate::app::BRANCH_EXAMPLE);
        assert!(workspace.pending_startup_command().is_none());
    }
}

#[test]
fn an_existing_catalog_fetch_is_reused_and_an_existing_catalog_can_start_immediately() {
    for received in [false, true] {
        let mut workspace = Workspace::default();
        assert!(matches!(
            workspace.rpc_provider_command(),
            Some(Command::RpcProviders { generation: 1 })
        ));
        if received {
            catalog(&mut workspace, 1, providers());
            assert_usdc(workspace.startup_command());
        } else {
            assert!(workspace.startup_command().is_none());
            assert_eq!(workspace.task.phase, TaskPhase::Idle);
            catalog(&mut workspace, 1, providers());
            assert_usdc(workspace.pending_startup_command());
        }
    }
}

#[test]
fn a_manual_submission_prevents_late_catalogs_from_starting_another_task() {
    let mut workspace = Workspace::default();
    workspace.startup_command();
    workspace.form.bytecode.bytecode = "600100".into();
    let Command::Submit { request, .. } = workspace.initial_command() else {
        panic!("expected the manual request")
    };
    assert!(matches!(request.input, AnalysisInput::Bytecode(input) if input.bytecode == "600100"));
    catalog(&mut workspace, 1, providers());
    assert!(workspace.pending_startup_command().is_none());
    assert!(workspace.startup_command().is_none());
}

#[test]
fn opening_the_editor_takes_control_even_if_the_catalog_is_already_ready() {
    let mut workspace = Workspace::default();
    workspace.startup_command();
    catalog(&mut workspace, 1, providers());
    workspace.form.open = true;
    assert!(workspace.pending_startup_command().is_none());
    workspace.form.open = false;
    assert!(workspace.pending_startup_command().is_none());
    assert!(workspace.startup_command().is_none());
}
