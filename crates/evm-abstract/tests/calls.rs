//! Frame, byte transport, rollback and shared-budget regression tests.

use alloy_primitives::{Address, U256};
use evm_abstract::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason, MachineEdgeKind, OutcomeKind, Status, WorldAnalysis,
        analyze_world,
    },
    domain::{Domain, Value},
    world::{Account, ByteArray, Code, Entry, World},
};

fn addr(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}
fn entry() -> Entry {
    Entry {
        address: addr(0x101),
        caller: addr(0x900),
        value: Value::constant(U256::from(42)),
        calldata: ByteArray::empty(),
        is_static: false,
    }
}
fn world(accounts: &[(u64, &str)]) -> World {
    let mut world = World::new(Fork::Osaka, "test:fixed-world");
    for (address, code) in accounts {
        let mut account = Account::from_hex(code, world.fork()).unwrap();
        account.balance = Value::constant(U256::from(1_000_000));
        world.insert(addr(*address), account).unwrap();
    }
    world
}
fn run(accounts: &[(u64, &str)]) -> WorldAnalysis {
    analyze_world(world(accounts), entry(), ExecutionConfig::default()).unwrap()
}
fn call(op: u8, target: u16, value: u8, input: u8, output: u8) -> String {
    let value = if matches!(op, 0xf1 | 0xf2) {
        format!("60{value:02x}")
    } else {
        String::new()
    };
    format!("60{output:02x}5f60{input:02x}5f{value}61{target:04x}6207a120{op:02x}")
}

#[test]
fn call_context_is_intrinsic_and_retained_across_each_call_kind() {
    for op in [0xf1, 0xf2, 0xf4, 0xfa] {
        let caller = format!("{}00", call(op, 0x200, 9, 0, 0));
        let analysis = run(&[(0x101, &caller), (0x200, "30333400")]);
        assert_eq!(analysis.status(), Status::Converged);
        let child = analysis
            .states()
            .iter()
            .find(|s| s.key.frames.len() == 2)
            .unwrap();
        let frame = child.entry.active();
        assert_eq!(frame.key.code_address, addr(0x200));
        assert_eq!(
            frame.key.address,
            if matches!(op, 0xf2 | 0xf4) {
                addr(0x101)
            } else {
                addr(0x200)
            }
        );
        assert_eq!(
            frame.key.caller,
            if op == 0xf4 { addr(0x900) } else { addr(0x101) }
        );
        assert_eq!(frame.key.is_static, op == 0xfa);
        assert_eq!(
            frame.call_value,
            Value::constant(U256::from(if op == 0xf4 {
                42
            } else if op == 0xfa {
                0
            } else {
                9
            }))
        );
        assert_eq!(
            child.exit_stack[0],
            Value::constant(U256::from_be_slice(frame.key.address.as_slice()))
        );
        assert_eq!(
            child.exit_stack[1],
            Value::constant(U256::from_be_slice(frame.key.caller.as_slice()))
        );
        assert_eq!(child.exit_stack[2], frame.call_value);
        assert!(
            analysis
                .edges()
                .iter()
                .any(|e| e.kind == MachineEdgeKind::Call)
        );
        assert!(
            analysis
                .edges()
                .iter()
                .any(|e| e.kind == MachineEdgeKind::Return)
        );
        assert!(
            analysis
                .edges()
                .iter()
                .any(|e| e.kind == MachineEdgeKind::Failure)
        );
    }
}

#[test]
fn calldata_is_copied_from_callers_memory_and_returned_bytes_feed_parent() {
    let caller = format!("60075f52{}505f5100", call(0xf1, 0x200, 0, 32, 32));
    let analysis = run(&[(0x101, &caller), (0x200, "5f355f5260205ff3")]);
    assert_eq!(analysis.status(), Status::Converged);
    let child = analysis
        .states()
        .iter()
        .find(|s| s.key.frames.len() == 2)
        .unwrap();
    assert_eq!(
        child
            .entry
            .active()
            .calldata
            .read_word(&Value::constant(U256::ZERO), Domain::default()),
        Value::constant(U256::from(7))
    );
    let resumed = analysis
        .states()
        .iter()
        .find(|s| s.key.frames.len() == 1 && s.active().block == 1)
        .unwrap();
    assert!(resumed.exit_stack[0].contains(U256::from(7)));
    assert!(
        resumed
            .entry
            .active()
            .returndata
            .len()
            .contains(U256::from(32))
    );
}

#[test]
fn another_call_clears_the_suspended_callers_previous_return_buffer() {
    let caller = format!(
        "{}50{}5000",
        call(0xf1, 0x200, 0, 0, 32),
        call(0xf1, 0x300, 0, 0, 0)
    );
    let graph = run(&[(0x101, &caller), (0x200, "602a5f5260205ff3"), (0x300, "00")]);
    assert_eq!(graph.status(), Status::Converged);
    let children: Vec<_> = graph
        .states()
        .iter()
        .filter(|s| s.key.frames.len() == 2 && s.active().code_address == addr(0x300))
        .collect();
    assert!(!children.is_empty());
    for child in children {
        assert_eq!(
            *child.entry.frames[0].returndata.len(),
            Value::constant(U256::ZERO)
        );
    }
}

#[test]
fn output_copy_preserves_uncopied_suffix_and_full_returndata_size() {
    let before = "aa".repeat(32);
    let caller = format!("7f{before}5f52{}503d5f5100", call(0xf1, 0x200, 0, 0, 32));
    let analysis = run(&[(0x101, &caller), (0x200, "60bb5f5360015ff3")]);
    let resumed = analysis
        .states()
        .iter()
        .find(|s| s.key.frames.len() == 1 && s.active().block == 1)
        .unwrap();
    assert!(resumed.exit_stack[0].contains(U256::from(1)));
    let mut expected = [0xaa; 32];
    expected[0] = 0xbb;
    assert!(resumed.exit_stack[1].contains(U256::from_be_bytes(expected)));
}

#[test]
fn child_revert_restores_storage_transient_balances_and_logs() {
    let caller = format!("60035f5560095f5d{}5000", call(0xf1, 0x200, 5, 0, 32));
    let callee = "60075f5560085f5d5f5fa0602a5f5260205ffd";
    let analysis = run(&[(0x101, &caller), (0x200, callee)]);
    assert_eq!(analysis.status(), Status::Converged);
    assert!(
        analysis
            .edges()
            .iter()
            .any(|e| e.kind == MachineEdgeKind::Revert)
    );
    for outcome in analysis
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
    {
        assert!(
            outcome
                .store
                .read(addr(0x101), &Value::constant(U256::ZERO), Domain::default())
                .contains(U256::from(3))
        );
        assert_eq!(
            outcome
                .store
                .read(addr(0x200), &Value::constant(U256::ZERO), Domain::default()),
            Value::constant(U256::ZERO)
        );
        assert_eq!(
            outcome.store.read_transient(
                addr(0x200),
                &Value::constant(U256::ZERO),
                Domain::default()
            ),
            Value::constant(U256::ZERO)
        );
        assert!(
            outcome
                .store
                .read_transient(addr(0x101), &Value::constant(U256::ZERO), Domain::default())
                .contains(U256::from(9))
        );
        assert_eq!(
            outcome.store.read_balance(addr(0x101)),
            Value::constant(U256::from(1_000_000))
        );
        assert!(outcome.store.possible_logs().is_empty());
    }
}

#[test]
fn static_child_write_fails_and_callcode_value_does_not_violate_static_mode() {
    let caller = format!("{}00", call(0xfa, 0x200, 0, 0, 0));
    let analysis = run(&[(0x101, &caller), (0x200, "60075f5500")]);
    assert!(
        analysis
            .edges()
            .iter()
            .any(|e| e.kind == MachineEdgeKind::Failure
                && analysis.states()[e.from].key.frames.len() == 2)
    );
    assert!(
        !analysis
            .edges()
            .iter()
            .any(|e| e.kind == MachineEdgeKind::Return
                && analysis.states()[e.from].key.frames.len() == 2)
    );
    let code = format!("{}00", call(0xf2, 0x200, 9, 0, 0));
    let mut entry = entry();
    entry.is_static = true;
    let analysis = analyze_world(
        world(&[(0x101, &code), (0x200, "3400")]),
        entry,
        ExecutionConfig::default(),
    )
    .unwrap();
    assert!(
        analysis
            .edges()
            .iter()
            .any(|e| e.kind == MachineEdgeKind::Return)
    );
}

#[test]
fn callbacks_observe_parent_writes_before_call_entry() {
    let a = "5f541561000f575f545f5260205ff35b60015f5560205f5f5f5f6102006207a120f1505f5160015560025f5500";
    let b = "60205f5f5f5f6101016207a120f15060205ff3";
    let analysis = run(&[(0x101, a), (0x200, b)]);
    assert_eq!(analysis.status(), Status::Converged);
    let callback = analysis
        .states()
        .iter()
        .find(|s| s.key.frames.len() == 3 && s.active().address == addr(0x101))
        .unwrap();
    assert_eq!(
        callback
            .entry
            .store
            .read(addr(0x101), &Value::constant(U256::ZERO), Domain::default()),
        Value::constant(U256::from(1))
    );
    assert!(
        analysis
            .outcomes()
            .iter()
            .filter(|o| o.kind == OutcomeKind::Return)
            .any(|o| o
                .store
                .read(
                    addr(0x101),
                    &Value::constant(U256::from(1)),
                    Domain::default()
                )
                .contains(U256::from(1)))
    );
}

#[test]
fn unknown_target_keeps_known_candidates_and_explicit_frontier() {
    let caller = "5f5f5f5f5f345af100";
    let mut entry = entry();
    entry.value = Value::top();
    let analysis = analyze_world(
        world(&[(0x101, caller), (0x200, "00")]),
        entry,
        ExecutionConfig {
            max_call_depth: 3,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis
            .frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::UnknownTarget)
    );
    assert!(
        analysis
            .states()
            .iter()
            .any(|s| s.key.frames.len() == 2 && s.active().code_address == addr(0x200))
    );
}

#[test]
fn missing_code_is_a_model_frontier_not_an_empty_success() {
    let target = 0x300;
    let caller = format!("{}00", call(0xf1, target, 0, 0, 0));
    let analysis = run(&[(0x101, &caller)]);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        !analysis
            .edges()
            .iter()
            .any(|e| e.kind == MachineEdgeKind::Call)
    );
    assert!(
        analysis
            .frontiers()
            .iter()
            .any(|f| matches!(f.reason, FrontierReason::MissingCode(_)))
    );
}

#[test]
fn delegation_retains_authority_storage_and_resolves_only_once() {
    let mut world = world(&[(0x101, "60075f553000")]);
    world
        .insert(
            addr(0x200),
            Account {
                code: Code::Delegation(addr(0x101)),
                ..Account::empty()
            },
        )
        .unwrap();
    let mut entry = entry();
    entry.address = addr(0x200);
    let analysis = analyze_world(world.clone(), entry.clone(), ExecutionConfig::default()).unwrap();
    assert_eq!(analysis.states()[0].active().code_address, addr(0x101));
    assert_eq!(analysis.states()[0].active().address, addr(0x200));
    assert!(analysis.outcomes().iter().any(|o| {
        o.kind == OutcomeKind::Return
            && o.store
                .read(addr(0x200), &Value::constant(U256::ZERO), Domain::default())
                .contains(U256::from(7))
    }));
    let mut nested = World::new(Fork::Osaka, "nested delegation");
    nested
        .insert(
            addr(0x101),
            Account {
                code: Code::Delegation(addr(0x300)),
                ..Account::empty()
            },
        )
        .unwrap();
    nested
        .insert(
            addr(0x200),
            Account {
                code: Code::Delegation(addr(0x101)),
                ..Account::empty()
            },
        )
        .unwrap();
    let analysis = analyze_world(nested, entry, ExecutionConfig::default()).unwrap();
    assert_eq!(analysis.status(), Status::Converged);
    assert!(
        !analysis
            .outcomes()
            .iter()
            .any(|o| o.kind == OutcomeKind::Return)
    );
}

#[test]
fn delegated_precompile_is_empty_while_direct_calls_use_native_execution() {
    let mut world = World::new(Fork::Osaka, "delegated native address");
    world
        .insert(
            addr(0x101),
            Account {
                code: Code::Delegation(addr(1)),
                ..Account::empty()
            },
        )
        .unwrap();
    let analysis = analyze_world(world, entry(), ExecutionConfig::default()).unwrap();
    assert_eq!(analysis.status(), Status::Converged);
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|o| o.kind == OutcomeKind::Return)
    );
}

#[test]
fn zero_size_returndatacopy_still_checks_source_offset() {
    let analysis = run(&[(0x101, "5f60015f3e00")]);
    assert_eq!(analysis.status(), Status::Converged);
    assert!(
        !analysis
            .outcomes()
            .iter()
            .any(|o| o.kind == OutcomeKind::Return)
    );
}

#[test]
fn shared_depth_and_work_limits_keep_typed_frontiers() {
    let recursive = format!("{}00", call(0xf1, 0x101, 0, 0, 0));
    let depth = analyze_world(
        world(&[(0x101, &recursive)]),
        entry(),
        ExecutionConfig {
            max_call_depth: 2,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(depth.status(), Status::Incomplete);
    assert!(
        depth
            .frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::CallDepth)
    );
    let work = analyze_world(
        world(&[(0x101, "600160020100")]),
        entry(),
        ExecutionConfig {
            max_work: 2,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(work.status(), Status::Incomplete);
    assert!(work.work() <= 2);
    assert!(
        work.frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::Work)
    );
    let memory = analyze_world(
        world(&[(0x101, "600160405200")]),
        entry(),
        ExecutionConfig {
            max_memory_bytes: 32,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert!(
        memory
            .frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::Memory)
    );
}
