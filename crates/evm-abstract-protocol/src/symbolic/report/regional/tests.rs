use super::block_evidence;
use crate::{
    AddressValue, CodeKind, DecimalU64, OutcomeKind, RegionalFrame, RegionalNode, RegionalTransfer,
    RegionalValidationKind,
};

#[test]
fn invalid_delegation_rejects_captured_block_and_visited_claims() {
    let owner = "0x0000000000000000000000000000000000000103".to_owned();
    let mut node = RegionalNode {
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
            input_revision: DecimalU64(1),
            current: true,
            complete: true,
            visited_block: false,
            successors: vec![],
            calls: vec![],
            outcomes: vec![OutcomeKind::Failure],
        }),
    };
    assert_eq!(block_evidence(&node), Ok(()));
    node.frame.basic_block = Some(0);
    assert_eq!(
        block_evidence(&node),
        Err(RegionalValidationKind::BlockVisit)
    );
    node.frame.basic_block = None;
    node.transfer.as_mut().unwrap().visited_block = true;
    assert_eq!(
        block_evidence(&node),
        Err(RegionalValidationKind::BlockVisit)
    );
}
