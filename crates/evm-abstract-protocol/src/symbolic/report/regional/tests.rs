use super::block_evidence;
use crate::{
    AddressValue, CodeKind, DecimalU64, OutcomeKind, RegionalFrame, RegionalNode, RegionalTransfer,
    RegionalValidationKind, TransferSemantics,
};

fn invalid_delegation_node() -> RegionalNode {
    let owner = "0x0000000000000000000000000000000000000103".to_owned();
    RegionalNode {
        id: 0,
        procedure: 0,
        frame: RegionalFrame {
            code_address: "0x0000000000000000000000000000000000000104".into(),
            code_hash: Some(format!("0x{}", "ab".repeat(32))),
            storage_address: owner.clone(),
            address_value: AddressValue::Concrete(owner),
            caller: AddressValue::Concrete("0x0000000000000000000000000000000000000102".into()),
            kind: CodeKind::InvalidDelegation,
            is_static: false,
            basic_block: None,
            stack_height: 0,
            context: vec![],
        },
        input_revision: DecimalU64(1),
        excluded: false,
        successors: vec![],
        transfer: Some(RegionalTransfer {
            semantics: TransferSemantics::Native,
            process: None,
            input_revision: DecimalU64(1),
            current: true,
            complete: true,
            visited_block: false,
            successors: vec![],
            calls: vec![],
            outcomes: vec![OutcomeKind::Failure],
        }),
    }
}

#[test]
fn invalid_delegation_rejects_captured_block_and_visited_claims() {
    let mut node = invalid_delegation_node();
    assert_eq!(block_evidence(&node, TransferSemantics::Native), Ok(()));
    node.frame.basic_block = Some(0);
    assert_eq!(
        block_evidence(&node, TransferSemantics::Native),
        Err(RegionalValidationKind::BlockVisit)
    );
    node.frame.basic_block = None;
    node.transfer.as_mut().unwrap().visited_block = true;
    assert_eq!(
        block_evidence(&node, TransferSemantics::Native),
        Err(RegionalValidationKind::BlockVisit)
    );
}

#[test]
fn interpretation_preserves_native_modes_without_fabricating_process_block_visits() {
    for kind in [
        CodeKind::Runtime,
        CodeKind::Initcode,
        CodeKind::Empty,
        CodeKind::Precompile,
        CodeKind::InvalidDelegation,
        CodeKind::AbstractEnvironment,
    ] {
        for semantics in [
            TransferSemantics::Native,
            TransferSemantics::Environment,
            TransferSemantics::KnownProcess,
        ] {
            let mut node = invalid_delegation_node();
            node.frame.kind = kind;
            let native_program = semantics == TransferSemantics::Native
                && matches!(kind, CodeKind::Runtime | CodeKind::Initcode);
            node.frame.basic_block = native_program.then_some(0);
            let transfer = node.transfer.as_mut().unwrap();
            transfer.semantics = semantics;
            transfer.visited_block = native_program;
            let compatible = match semantics {
                TransferSemantics::Native => kind != CodeKind::AbstractEnvironment,
                TransferSemantics::Environment => kind == CodeKind::AbstractEnvironment,
                TransferSemantics::KnownProcess => kind == CodeKind::Runtime,
            };
            assert_eq!(
                block_evidence(&node, semantics),
                if compatible {
                    Ok(())
                } else {
                    Err(RegionalValidationKind::Semantics)
                },
                "{kind:?} / {semantics:?}"
            );
            if compatible && !native_program {
                node.transfer.as_mut().unwrap().visited_block = true;
                assert_eq!(
                    block_evidence(&node, semantics),
                    Err(RegionalValidationKind::BlockVisit)
                );
            }
        }
    }
}

#[test]
fn process_rules_cannot_be_native_or_complete_with_unresolved_obligations() {
    let mut node = invalid_delegation_node();
    node.frame.kind = CodeKind::Runtime;
    let transfer = node.transfer.as_mut().unwrap();
    transfer.semantics = TransferSemantics::KnownProcess;
    transfer.process = Some(crate::ProcessTransfer {
        repeat: true,
        actions: vec![crate::ProcessAction::StaticCall],
        unresolved: vec![crate::ProcessBoundary::ResidualOwner(
            crate::ProcessAction::StaticCall,
        )],
    });
    assert_eq!(
        block_evidence(&node, TransferSemantics::KnownProcess),
        Err(RegionalValidationKind::Semantics)
    );
    node.transfer.as_mut().unwrap().complete = false;
    assert_eq!(
        block_evidence(&node, TransferSemantics::KnownProcess),
        Ok(())
    );
    node.transfer.as_mut().unwrap().semantics = TransferSemantics::Native;
    node.frame.basic_block = Some(0);
    assert_eq!(
        block_evidence(&node, TransferSemantics::Native),
        Err(RegionalValidationKind::Semantics)
    );
}
