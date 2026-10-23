use super::{address, budget, catalogue, domain, prepare, store, word};
use crate::{
    domain::NumericValue,
    world::{
        AddressInput, InputScope,
        process::{
            CallRoles, ProcessAction, ProcessCallOperands, ProcessOwner, ProcessOwnerError,
            ProcessTargetAddress,
        },
    },
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

#[test]
fn call_family_roles_retain_owner_caller_value_and_staticness() {
    let domain = domain();
    let parent = ProcessCallOperands {
        address: word(0xaaa),
        caller: word(0x900),
        value: word(17),
    };
    for parent_static in [false, true] {
        for action in [
            ProcessAction::Call,
            ProcessAction::StaticCall,
            ProcessAction::DelegateCall,
            ProcessAction::CallCode,
        ] {
            let roles = CallRoles::for_action(action, parent_static).unwrap();
            let operands = roles
                .bind(
                    ProcessTargetAddress::Exact(address(0x200)),
                    &parent,
                    domain.borrowed(),
                    &mut budget(),
                )
                .unwrap();
            let inherited = matches!(
                action,
                ProcessAction::DelegateCall | ProcessAction::CallCode
            );
            assert_eq!(
                roles.child_owner(ProcessOwner::Residual, ProcessOwner::Named(address(0x200))),
                if inherited {
                    ProcessOwner::Residual
                } else {
                    ProcessOwner::Named(address(0x200))
                }
            );
            assert_eq!(
                roles.is_static,
                parent_static || action == ProcessAction::StaticCall
            );
            assert_eq!(
                operands.address.singleton(),
                Some(U256::from(if inherited { 0xaaa } else { 0x200 }))
            );
            assert_eq!(
                operands.caller.singleton(),
                Some(U256::from(if action == ProcessAction::DelegateCall {
                    0x900
                } else {
                    0xaaa
                }))
            );
            if action == ProcessAction::DelegateCall {
                assert_eq!(operands.value.singleton(), Some(U256::from(17)));
            } else if roles.is_static && action != ProcessAction::CallCode {
                assert_eq!(operands.value.singleton(), Some(U256::ZERO));
            } else {
                assert!(operands.value.numeric().is_top());
                assert!(operands.value.contains(U256::from(9)));
            }
        }
    }
    for action in [ProcessAction::Create, ProcessAction::Create2] {
        assert_eq!(
            CallRoles::for_action(action, false),
            Err(ProcessOwnerError::Allocation)
        );
    }
}

#[test]
fn independent_residual_calls_cover_equal_and_different_owners_without_global_symbols() {
    let domain = domain();
    let seed = ProcessCallOperands {
        address: word(0x100),
        caller: word(0x900),
        value: word(0),
    };
    let roles = CallRoles::for_action(ProcessAction::Call, false).unwrap();
    let parent = roles
        .bind(
            ProcessTargetAddress::Independent,
            &seed,
            domain.borrowed(),
            &mut budget(),
        )
        .unwrap();
    let child = roles
        .bind(
            ProcessTargetAddress::Independent,
            &parent,
            domain.borrowed(),
            &mut budget(),
        )
        .unwrap();
    for operand in [&parent.address, &child.address, &child.caller] {
        assert!(operand.contains(U256::from(0xaaa)) && operand.contains(U256::from(0xbbb)));
        assert!(!operand.contains(U256::ONE << 160usize));
    }
    assert!(!child.address.same_identity(&child.caller));
    assert!(child.address.expression().is_none() && child.caller.expression().is_none());
    let equality = domain
        .borrowed()
        .apply_budgeted(opcode::EQ, &[child.address, child.caller], &mut budget())
        .unwrap()
        .value;
    assert!(equality.contains(U256::ZERO) && equality.contains(U256::ONE));
}

#[test]
fn symbolic_logical_address_can_have_a_named_physical_owner() {
    let domain = domain();
    let catalogue = catalogue();
    let logical = AddressInput::unknown_to().scoped_value::<NumericValue>(Some(0x123));
    let parent = ProcessCallOperands {
        address: logical.clone(),
        caller: word(0x900),
        value: word(0),
    };
    let input = prepare(
        &store(&domain),
        &catalogue,
        ProcessOwner::Named(address(0x100)),
        &domain,
    );
    assert_eq!(input.key().owner, ProcessOwner::Named(address(0x100)));
    let bound = CallRoles::for_action(ProcessAction::Call, false)
        .unwrap()
        .bind(
            ProcessTargetAddress::Operand(&logical),
            &parent,
            domain.borrowed(),
            &mut budget(),
        )
        .unwrap();
    assert!(bound.address.same_identity(&logical));
    assert_eq!(bound.address.singleton(), None);
    assert!(bound.address.contains(U256::from(0xaaa)));
}

#[test]
fn native_input_scopes_and_inherited_roles_survive_without_equating_residual_call_targets() {
    let domain = domain();
    let scope = InputScope::default();
    let other_scope = InputScope::default();
    let parent = ProcessCallOperands {
        address: AddressInput::unknown_to().scoped_value::<NumericValue>(scope.id()),
        caller: AddressInput::unknown_caller().scoped_value::<NumericValue>(scope.id()),
        value: word(17),
    };
    let other_address = AddressInput::unknown_to().scoped_value::<NumericValue>(other_scope.id());
    assert!(!parent.address.same_identity(&other_address));
    let delegate = CallRoles::for_action(ProcessAction::DelegateCall, false)
        .unwrap()
        .bind(
            ProcessTargetAddress::Independent,
            &parent,
            domain.borrowed(),
            &mut budget(),
        )
        .unwrap();
    assert!(delegate.address.same_identity(&parent.address));
    assert!(delegate.caller.same_identity(&parent.caller));
    assert_eq!(delegate.value.singleton(), Some(U256::from(17)));
    let callcode = CallRoles::for_action(ProcessAction::CallCode, false)
        .unwrap()
        .bind(
            ProcessTargetAddress::Independent,
            &parent,
            domain.borrowed(),
            &mut budget(),
        )
        .unwrap();
    assert!(callcode.address.same_identity(&parent.address));
    assert!(callcode.caller.same_identity(&parent.address));
    let child = CallRoles::for_action(ProcessAction::Call, false)
        .unwrap()
        .bind(
            ProcessTargetAddress::Independent,
            &parent,
            domain.borrowed(),
            &mut budget(),
        )
        .unwrap();
    assert!(child.caller.same_identity(&parent.address));
    assert!(!child.address.same_identity(&parent.address));
    assert!(!child.address.same_identity(&other_address));
    let equality = domain
        .borrowed()
        .apply_budgeted(opcode::EQ, &[child.address, child.caller], &mut budget())
        .unwrap()
        .value;
    assert!(equality.contains(U256::ZERO) && equality.contains(U256::ONE));
}
