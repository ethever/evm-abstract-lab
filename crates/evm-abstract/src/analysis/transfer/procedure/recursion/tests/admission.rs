//! Admission depends on repeated live control-flow edges, never payload values.

#[cfg(test)]
use crate::analysis::testing::ByteArray;

use super::super::is_recursive_entry;
use crate::analysis::{
    ChildFrame, Continuation, FrameCode, MachinePayload,
    machine::fixtures::{address, payload, word},
};
use alloy_primitives::B256;

fn enter(payload: &mut MachinePayload, site: usize, return_block: usize, code: u64, owner: u64) {
    payload.active_mut().key.basic_block_index = site;
    let mut state = payload.active().clone();
    state.key.code_address = address(code);
    state.key.address = address(owner);
    state.key.address_value = address(owner).into();
    state.key.basic_block_index = 0;
    state.stack.clear();
    payload.call_stack.push_child(ChildFrame {
        state,
        continuation: Continuation {
            return_block: Some(return_block),
            output_offset: word(0),
            output_size: word(0),
            creation: None,
        },
    });
}

fn repeated() -> MachinePayload {
    let mut out =
        payload(crate::analysis::testing::product(crate::domain::DomainSpec::default()).borrowed());
    enter(&mut out, 3, 4, 0x101, 0x101);
    enter(&mut out, 3, 4, 0x101, 0x101);
    out
}

#[test]
fn one_self_child_is_not_a_repeated_active_call_edge() {
    let mut out =
        payload(crate::analysis::testing::product(crate::domain::DomainSpec::default()).borrowed());
    assert!(!is_recursive_entry(&out));
    enter(&mut out, 3, 4, 0x101, 0x101);
    assert!(!is_recursive_entry(&out));
    enter(&mut out, 3, 4, 0x101, 0x101);
    assert!(is_recursive_entry(&out));
    assert_eq!(out.call_stack.depth(), 3);
}

#[test]
fn returned_self_children_do_not_accumulate_recursion_evidence() {
    let mut out =
        payload(crate::analysis::testing::product(crate::domain::DomainSpec::default()).borrowed());
    for _ in 0..8 {
        enter(&mut out, 3, 4, 0x101, 0x101);
        assert!(!is_recursive_entry(&out));
        assert!(out.call_stack.pop_child().is_some());
        assert_eq!(out.call_stack.depth(), 1);
    }
}

#[test]
fn mutual_recursion_folds_when_the_first_call_edge_repeats() {
    let mut out =
        payload(crate::analysis::testing::product(crate::domain::DomainSpec::default()).borrowed());
    enter(&mut out, 3, 4, 0x102, 0x102);
    assert!(!is_recursive_entry(&out));
    enter(&mut out, 7, 8, 0x101, 0x101);
    assert!(!is_recursive_entry(&out));
    enter(&mut out, 3, 4, 0x102, 0x102);
    assert!(is_recursive_entry(&out));
    assert_eq!(out.call_stack.depth(), 4);
}

#[test]
fn caller_value_calldata_and_output_range_are_not_edge_identity() {
    let mut out = repeated();
    let child = out.call_stack.children_mut().last_mut().unwrap();
    child.state.key.caller = address(0x900).into();
    child.state.call_value = word(17);
    child.state.calldata = ByteArray::exact(&[1, 2, 3]);
    child.state.key.jump_history = vec![2, 9, 3];
    child.continuation.output_offset = word(64);
    child.continuation.output_size = word(128);
    assert!(is_recursive_entry(&out));
}

#[test]
fn repeated_edge_keeps_both_execution_and_storage_contexts_distinct() {
    let original = repeated();
    for parent in [false, true] {
        for variant in 0..7 {
            let mut out = original.clone();
            let children = out.call_stack.children_mut();
            let frame = &mut children[usize::from(!parent)].state;
            match variant {
                0 => frame.key.code_address = address(0x102),
                1 => frame.key.code_hash = B256::repeat_byte(7),
                2 => frame.key.mode = FrameCode::Empty,
                3 => frame.key.address = address(0x102),
                4 => frame.key.address_value = address(0x102).into(),
                5 => frame.key.is_static = true,
                6 => frame.code = FrameCode::Empty,
                _ => unreachable!(),
            }
            assert!(
                !is_recursive_entry(&out),
                "parent={parent}, variant={variant}"
            );
        }
    }
}

#[test]
fn distinct_call_sites_and_continuations_do_not_match() {
    let mut out = repeated();
    out.call_stack.children_mut()[0].state.key.basic_block_index = 5;
    assert!(!is_recursive_entry(&out));
    let mut out = repeated();
    out.call_stack.children_mut()[1].continuation.return_block = Some(5);
    assert!(!is_recursive_entry(&out));
}

#[test]
fn creation_and_nonentry_children_cannot_trigger_known_folding() {
    for variant in 0..4 {
        let mut out = repeated();
        let children = out.call_stack.children_mut();
        match variant {
            0 => children[1].continuation.creation = Some(address(0x102)),
            1 => children[0].continuation.creation = Some(address(0x102)),
            2 => children[1].state.key.basic_block_index = 1,
            3 => children[1].state.stack.push(word(1)),
            _ => unreachable!(),
        }
        assert!(!is_recursive_entry(&out), "variant={variant}");
    }
}
