//! Typed frame roles and the call stack's root/child ownership boundary.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        CallStack, ChildFrame, Continuation, ExecutionConfig, MachineEdgeKind, OutcomeKind,
        RootFrame, Status, WorldAnalysis, analyze_world,
    },
    domain::{Domain, Value},
    ssa,
    world::{Account, ByteArray, Entry, Store, World},
};

fn root_frame() -> RootFrame {
    let address = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "typed call stack fixture");
    world.insert(address, Account::empty()).unwrap();
    analyze_world(
        world,
        Entry {
            address,
            environment: evm_abstract::world::EvmEnvironment {
                to: (address).into(),
                caller: (Address::repeat_byte(0x22)).into(),
                value: Value::constant(U256::ZERO),
                calldata: ByteArray::empty(),
                is_static: false,
                ..evm_abstract::world::EvmEnvironment::default()
            },
        },
        ExecutionConfig::default(),
    )
    .unwrap()
    .states()[0]
        .entry
        .call_stack
        .root()
        .clone()
}

fn continuation(block: usize) -> Continuation {
    Continuation {
        return_block: Some(block),
        output_offset: Value::constant(U256::from(block)),
        output_size: Value::constant(U256::from(32)),
        creation: None,
    }
}

#[test]
fn nested_stack_keeps_the_root_and_resumes_each_parent() {
    let root = root_frame();
    let mut stack = CallStack::new(root.clone());
    assert_eq!(stack.depth(), 1);
    assert!(stack.pop_child().is_none());
    assert!(stack.parent().is_none());
    assert!(stack.active_child().is_none());

    let mut first_state = root.state.clone();
    first_state.stack.push(Value::constant(U256::from(1)));
    let first = ChildFrame {
        state: first_state,
        continuation: continuation(1),
    };
    stack.push_child(first.clone());
    assert_eq!(stack.parent(), Some(&root.state));

    let mut second_state = root.state.clone();
    second_state.stack.push(Value::constant(U256::from(2)));
    let second = ChildFrame {
        state: second_state,
        continuation: continuation(2),
    };
    stack.push_child(second.clone());
    assert_eq!(stack.depth(), 3);
    assert_eq!(stack.root(), &root);
    assert_eq!(stack.children(), &[first.clone(), second.clone()]);
    assert_eq!(stack.parent(), Some(&first.state));
    assert_eq!(stack.active_child(), Some(&second));
    assert_eq!(
        stack.iter().collect::<Vec<_>>(),
        vec![&root.state, &first.state, &second.state]
    );

    stack
        .active_mut()
        .stack
        .push(Value::constant(U256::from(3)));
    assert_eq!(stack.pop_child().unwrap().state.stack.len(), 2);
    assert_eq!(stack.active(), &first.state);
    assert_eq!(stack.pop_child(), Some(first));
    assert!(stack.pop_child().is_none());
    assert_eq!(stack.depth(), 1);
    assert_eq!(stack.active(), &root.state);
    assert_eq!(stack.root(), &root);
}

#[test]
fn changing_frame_role_preserves_execution_and_rollback_state() {
    let root = root_frame();
    let return_to = continuation(7);
    let child = root.clone().into_child(return_to.clone());
    assert_eq!(child.state, root.state);
    assert_eq!(child.continuation, return_to);
    let (boundary, saved_return_to) = child.into_root();
    assert_eq!(boundary, root);
    assert_eq!(saved_return_to, return_to);
    assert_eq!(boundary.state.saved_store, root.state.saved_store);
}

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn assert_equivalent_store(actual: &Store, expected: &Store) {
    // Joins materialize default lifecycle flags. Normalize both stores so
    // omitted false flags and explicit false flags compare as the same state.
    let domain = Domain::default();
    assert_eq!(actual.join(actual, domain), expected.join(expected, domain));
}

fn nested_analysis(root_reverts: bool) -> WorldAnalysis {
    let end = if root_reverts { "5f5ffd" } else { "00" };
    let root = format!("60035f555f5f5f5f5f6102006207a120f150{end}");
    let child = "60075f555f5f5f5f5f6103006207a120f1505f5ffd";
    let grandchild = "60095f55600b5f5d00";
    let mut world = World::new(Fork::Osaka, "nested checkpoint fixture");
    for (number, code) in [(0x101, root.as_str()), (0x200, child), (0x300, grandchild)] {
        world
            .insert(
                address(number),
                Account::from_hex(code, Fork::Osaka).unwrap(),
            )
            .unwrap();
    }
    analyze_world(
        world,
        Entry {
            address: address(0x101),
            environment: evm_abstract::world::EvmEnvironment {
                to: (address(0x101)).into(),
                caller: (address(0x900)).into(),
                value: Value::constant(U256::ZERO),
                calldata: ByteArray::empty(),
                is_static: false,
                ..evm_abstract::world::EvmEnvironment::default()
            },
        },
        ExecutionConfig::default(),
    )
    .unwrap()
}

#[test]
fn nested_calls_preserve_roles_and_restore_the_childs_checkpoint() {
    let analysis = nested_analysis(false);
    assert_eq!(analysis.status(), Status::Converged);
    ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
    let zero = Value::constant(U256::ZERO);
    let domain = Domain::default();
    let root_checkpoint = &analysis.states()[0]
        .entry
        .call_stack
        .root()
        .state
        .saved_store;
    let mut saw_grandchild = false;
    for state in analysis.states() {
        let stack = &state.entry.call_stack;
        assert_eq!(stack.root().state.key.address, address(0x101));
        assert_equivalent_store(
            stack.root().state.saved_store.state(),
            root_checkpoint.state(),
        );
        assert_eq!(stack.depth(), 1 + stack.children().len());
        assert_eq!(
            state.key.frames,
            stack
                .iter()
                .map(|frame| frame.key.clone())
                .collect::<Vec<_>>()
        );
        if stack.depth() == 3 {
            saw_grandchild = true;
            let child_checkpoint = stack.children()[0].state.saved_store.state();
            assert_eq!(
                child_checkpoint.read(address(0x101), &zero, domain),
                Value::constant(U256::from(3))
            );
            assert_eq!(child_checkpoint.read(address(0x200), &zero, domain), zero);
            let grandchild = stack.active_child().unwrap();
            assert_eq!(grandchild.continuation.return_block, Some(1));
            assert_eq!(stack.parent(), Some(&stack.children()[0].state));
            let checkpoint = grandchild.state.saved_store.state();
            assert_eq!(
                checkpoint.read(address(0x200), &zero, domain),
                Value::constant(U256::from(7))
            );
            assert_eq!(checkpoint.read(address(0x300), &zero, domain), zero);
        }
    }
    assert!(saw_grandchild);

    let reverts: Vec<_> = analysis
        .edges()
        .iter()
        .filter(|edge| edge.kind == MachineEdgeKind::Revert)
        .collect();
    assert!(!reverts.is_empty());
    for edge in reverts {
        let source = &analysis.states()[edge.from];
        let resumed = &analysis.states()[edge.to].entry;
        assert_eq!(source.entry.call_stack.depth(), 2);
        assert_eq!(resumed.call_stack.depth(), 1);
        let checkpoint = source
            .entry
            .call_stack
            .active_child()
            .unwrap()
            .state
            .saved_store
            .state();
        assert_equivalent_store(&resumed.store, checkpoint);
        assert_eq!(
            resumed.store.read(address(0x101), &zero, domain),
            Value::constant(U256::from(3))
        );
        assert_eq!(resumed.store.read(address(0x200), &zero, domain), zero);
        assert_eq!(resumed.store.read(address(0x300), &zero, domain), zero);
        assert_eq!(
            resumed.store.read_transient(address(0x300), &zero, domain),
            zero
        );
    }
    // The deeper successful write exists before B rolls the whole subtree back.
    assert!(analysis.states().iter().any(|state| {
        state.entry.call_stack.depth() == 2
            && state
                .entry
                .store
                .read(address(0x300), &zero, domain)
                .contains(U256::from(9))
    }));
}

#[test]
fn root_revert_restores_the_transaction_checkpoint_after_children_finish() {
    let analysis = nested_analysis(true);
    assert_eq!(analysis.status(), Status::Converged);
    let checkpoint = analysis.states()[0]
        .entry
        .call_stack
        .root()
        .state
        .saved_store
        .state();
    let reverts: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Revert)
        .collect();
    assert!(!reverts.is_empty());
    for outcome in reverts {
        assert_equivalent_store(&outcome.store, checkpoint);
    }
}

#[test]
fn serialized_execution_data_distinguishes_root_and_child_roles() {
    let analysis = nested_analysis(false);
    let state = analysis
        .states()
        .iter()
        .find(|state| state.entry.call_stack.depth() == 3)
        .unwrap();
    let entry = serde_json::to_value(&state.entry).unwrap();
    assert!(entry.get("frames").is_none());
    let stack = &entry["call_stack"];
    assert!(stack["root"]["state"]["saved_store"].is_object());
    assert!(stack["root"].get("continuation").is_none());
    for child in stack["children"].as_array().unwrap() {
        assert!(child["state"]["saved_store"].is_object());
        assert!(child["continuation"].is_object());
        assert!(child["state"].get("continuation").is_none());
    }
}
