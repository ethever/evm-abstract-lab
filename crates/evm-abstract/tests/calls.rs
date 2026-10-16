//! Frame, byte transport, rollback and shared-budget regression tests.

use alloy_primitives::{Address, U256};
use evm_abstract::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason, MachineEdgeKind, OutcomeKind, Status, WorldAnalysis,
        analyze_world,
    },
    domain::{AbstractValue, Domain, provenance::Origin},
    world::{Account, ByteArray, Code, Entry, World},
};

fn addr(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

// 上下文数值来自地址/CALLVALUE，不能要求它带有直接字面常量的来源。
fn assert_numeric_eq(actual: &AbstractValue, expected: &AbstractValue) {
    assert_eq!(actual.constants(), expected.constants());
    assert_eq!(actual.known_bits(), expected.known_bits());
    assert_eq!(actual.interval(), expected.interval());
    assert_eq!(actual.congruence(), expected.congruence());
    assert_eq!(actual.may_be_zero(), expected.may_be_zero());
    assert_eq!(actual.may_be_nonzero(), expected.may_be_nonzero());
}
fn entry() -> Entry {
    Entry {
        address: addr(0x101),
        environment: evm_abstract::world::EvmEnvironment {
            to: (addr(0x101)).into(),
            caller: (addr(0x900)).into(),
            value: AbstractValue::constant(U256::from(42)),
            calldata: ByteArray::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironment::default()
        },
    }
}
fn world(accounts: &[(u64, &str)]) -> World {
    let mut world = World::new(Fork::Osaka, "test:fixed-world");
    for (address, code) in accounts {
        let mut account = Account::from_hex(code, world.fork()).unwrap();
        account.balance = AbstractValue::constant(U256::from(1_000_000));
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
fn static_value_call_faults_before_an_unrepresentable_memory_access() {
    let code = format!("5f5f60017f{}60016102005af100", "ff".repeat(32));
    let world = world(&[(0x101, &code), (0x200, "00")]);
    let mut entry = entry();
    entry.environment.is_static = true;
    let analysis = analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(analysis.frontiers().is_empty());
    assert!(
        analysis
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind == OutcomeKind::Failure)
    );
    assert!(
        analysis
            .edges()
            .iter()
            .all(|edge| edge.kind != MachineEdgeKind::Call)
    );
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
            if op == 0xf4 {
                addr(0x900).into()
            } else {
                addr(0x101).into()
            }
        );
        assert_eq!(frame.key.is_static, op == 0xfa);
        assert_numeric_eq(
            &frame.call_value,
            &AbstractValue::constant(U256::from(if op == 0xf4 {
                42
            } else if op == 0xfa {
                0
            } else {
                9
            })),
        );
        assert_numeric_eq(
            &child.exit_stack[0],
            &AbstractValue::constant(U256::from_be_slice(frame.key.address.as_slice())),
        );
        assert_numeric_eq(
            &child.exit_stack[1],
            &AbstractValue::constant(U256::from_be_slice(
                frame.key.caller.as_concrete().unwrap().as_slice(),
            )),
        );
        assert_numeric_eq(&child.exit_stack[2], &frame.call_value);
        for (value, origin) in [
            (&child.exit_stack[0], Origin::Address),
            (&child.exit_stack[1], Origin::Address),
            (&child.exit_stack[2], Origin::CallValue),
        ] {
            assert!(
                value
                    .provenance()
                    .origins()
                    .sources()
                    .unwrap()
                    .contains(&origin)
            );
        }
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
    let loaded = child
        .entry
        .active()
        .calldata
        .read_word(&AbstractValue::constant(U256::ZERO), Domain::default());
    assert_numeric_eq(&loaded, &AbstractValue::constant(U256::from(7)));
    assert!(
        loaded
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Arithmetic)
    );
    let resumed: Vec<_> = analysis
        .states()
        .iter()
        .filter(|s| s.key.frames.len() == 1 && s.active().basic_block_index == 1)
        .collect();
    for success in [false, true] {
        let states: Vec<_> = resumed
            .iter()
            .filter(|s| {
                s.entry.active().stack.last().unwrap().singleton()
                    == Some(U256::from(u8::from(success)))
            })
            .collect();
        assert!(!states.is_empty(), "CALL status {success} was not covered");
        for state in states {
            assert_numeric_eq(
                &state.exit_stack[0],
                &AbstractValue::constant(U256::from(7)),
            );
            assert_eq!(
                state.entry.active().returndata.len().singleton(),
                Some(U256::from(if success { 32 } else { 0 }))
            );
        }
    }
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
            *child.entry.call_stack.root().state.returndata.len(),
            AbstractValue::constant(U256::ZERO)
        );
    }
}

#[test]
fn output_copy_preserves_uncopied_suffix_and_full_returndata_size() {
    let before = "aa".repeat(32);
    let caller = format!("7f{before}5f52{}503d5f5100", call(0xf1, 0x200, 0, 0, 32));
    let analysis = run(&[(0x101, &caller), (0x200, "60bb5f5360015ff3")]);
    assert_eq!(analysis.status(), Status::Converged);
    let resumed: Vec<_> = analysis
        .states()
        .iter()
        .filter(|s| s.key.frames.len() == 1 && s.active().basic_block_index == 1)
        .collect();
    for success in [false, true] {
        let states: Vec<_> = resumed
            .iter()
            .filter(|s| {
                s.entry.active().stack.last().unwrap().singleton()
                    == Some(U256::from(u8::from(success)))
            })
            .collect();
        assert!(!states.is_empty(), "CALL status {success} was not covered");
        let mut expected = [0xaa; 32];
        if success {
            expected[0] = 0xbb;
        }
        for state in states {
            assert_eq!(
                state.exit_stack[0].singleton(),
                Some(U256::from(u8::from(success)))
            );
            assert_numeric_eq(
                &state.exit_stack[1],
                &AbstractValue::constant(U256::from_be_bytes(expected)),
            );
        }
    }
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
    let domain = Domain::default();
    let zero = AbstractValue::constant(U256::ZERO);
    let revert_edges: Vec<_> = analysis
        .edges()
        .iter()
        .filter(|edge| {
            edge.kind == MachineEdgeKind::Revert
                && analysis.states()[edge.from].key.frames.len() == 2
                && analysis.states()[edge.to].key.frames.len() == 1
        })
        .collect();
    assert!(!revert_edges.is_empty());
    for edge in revert_edges {
        let child = &analysis.states()[edge.from];
        let checkpoint = child.entry.active().saved_store.state();
        let effects = &child.exit.as_ref().unwrap().store;
        assert_eq!(
            effects.read(addr(0x200), &zero, domain),
            AbstractValue::constant(U256::from(7))
        );
        assert_eq!(
            effects.read_transient(addr(0x200), &zero, domain),
            AbstractValue::constant(U256::from(8))
        );
        assert_eq!(effects.possible_logs().len(), 1);
        let receipt = child.execution_evidence().unwrap();
        assert!(receipt.is_current());
        assert!(
            receipt
                .successors()
                .contains(&(edge.to, MachineEdgeKind::Revert))
        );

        let resumed = &analysis.states()[edge.to].entry;
        // Compare the complete state before transaction finalization clears TSTORE.
        assert_eq!(&resumed.store, checkpoint);
        assert_eq!(
            resumed.store.read(addr(0x101), &zero, domain),
            AbstractValue::constant(U256::from(3))
        );
        assert_eq!(
            resumed.store.read_transient(addr(0x101), &zero, domain),
            AbstractValue::constant(U256::from(9))
        );
        assert_eq!(resumed.store.read(addr(0x200), &zero, domain), zero);
        assert_eq!(
            resumed.store.read_transient(addr(0x200), &zero, domain),
            zero
        );
        assert_eq!(resumed.active().stack.last().unwrap(), &zero);
        // The same resumption also receives exceptional CALL failures with an
        // empty buffer. Preserve the exact joined product for both alternatives.
        let length = domain.join(&zero, &AbstractValue::constant(U256::from(32)));
        assert_eq!(
            resumed.active().returndata.len().numeric(),
            length.numeric()
        );
        assert_eq!(
            resumed.active().returndata.len().provenance(),
            length.provenance()
        );
        assert_eq!(
            child
                .exit
                .as_ref()
                .unwrap()
                .active()
                .memory
                .read_word(&zero, domain),
            AbstractValue::constant(U256::from(42))
        );
        let copied = resumed.active().memory.read_word(&zero, domain);
        let expected_copy = domain
            .join(&zero, &AbstractValue::constant(U256::from(42)))
            .with_origin(Origin::Arithmetic);
        assert_eq!(copied.numeric(), expected_copy.numeric());
        assert_eq!(copied.provenance(), expected_copy.provenance());
        assert!(copied.expression().is_some());
        assert_eq!(resumed.active().memory.read_word(&zero, domain), copied);
        assert_eq!(
            resumed.store.read_balance(addr(0x101)),
            AbstractValue::constant(U256::from(1_000_000))
        );
        assert_eq!(
            resumed.store.read_balance(addr(0x200)),
            AbstractValue::constant(U256::from(1_000_000))
        );
        assert!(resumed.store.possible_logs().is_empty());
    }
    let returns: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
        .collect();
    assert!(!returns.is_empty());
    for outcome in returns {
        assert_eq!(
            outcome.store.read(
                addr(0x101),
                &AbstractValue::constant(U256::ZERO),
                Domain::default()
            ),
            AbstractValue::constant(U256::from(3))
        );
        assert_eq!(
            outcome.store.read(
                addr(0x200),
                &AbstractValue::constant(U256::ZERO),
                Domain::default()
            ),
            AbstractValue::constant(U256::ZERO)
        );
        assert_eq!(
            outcome.store.read_transient(
                addr(0x200),
                &AbstractValue::constant(U256::ZERO),
                Domain::default()
            ),
            AbstractValue::constant(U256::ZERO)
        );
        assert_eq!(
            outcome.store.read_transient(
                addr(0x101),
                &AbstractValue::constant(U256::ZERO),
                Domain::default()
            ),
            zero
        );
        assert!(outcome.store.transient_slots().is_empty());
        assert_eq!(
            outcome.store.read_balance(addr(0x101)),
            AbstractValue::constant(U256::from(1_000_000))
        );
        assert_eq!(
            outcome.store.read_balance(addr(0x200)),
            AbstractValue::constant(U256::from(1_000_000))
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
    entry.environment.is_static = true;
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
        callback.entry.store.read(
            addr(0x101),
            &AbstractValue::constant(U256::ZERO),
            Domain::default()
        ),
        AbstractValue::constant(U256::from(1))
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
                    &AbstractValue::constant(U256::from(1)),
                    Domain::default()
                )
                .contains(U256::from(1)))
    );
}

#[test]
fn unknown_target_keeps_known_candidates_and_models_the_remainder() {
    let caller = "5f5f5f5f5f345af100";
    let mut entry = entry();
    entry.environment.value = AbstractValue::top();
    let analysis = analyze_world(
        world(&[(0x101, caller), (0x200, "00")]),
        entry,
        ExecutionConfig {
            max_call_depth: 3,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    // The known self target now closes through a relative procedure instead
    // of exhausting the physical depth limit while the unknown remainder runs.
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    assert!(!analysis.procedure_summaries().is_empty());
    assert!(
        analysis
            .procedure_summaries()
            .iter()
            .all(|summary| summary.closed)
    );
    assert!(
        analysis
            .states()
            .iter()
            .any(|s| s.key.frames.len() == 2 && s.active().code_address == addr(0x200))
    );
}

#[test]
fn missing_code_uses_a_closed_interaction_summary_not_an_empty_success() {
    let target = 0x300;
    let caller = format!("{}00", call(0xf1, target, 0, 0, 0));
    let analysis = run(&[(0x101, &caller)]);
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    assert!(
        analysis
            .edges()
            .iter()
            .any(|e| e.kind == MachineEdgeKind::Call)
    );
    assert!(
        analysis
            .states()
            .iter()
            .any(|state| state.active().code_address == addr(target.into())
                && state.active().mode == evm_abstract::analysis::FrameCode::Unknown)
    );
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| matches!(edge.kind, MachineEdgeKind::Summary(OutcomeKind::Return)))
    );
    assert!(!analysis.procedure_summaries().is_empty());
    assert!(
        analysis
            .procedure_summaries()
            .iter()
            .all(|summary| summary.closed)
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
    entry.environment.to = addr(0x200).into();
    let analysis = analyze_world(world.clone(), entry.clone(), ExecutionConfig::default()).unwrap();
    assert_eq!(analysis.states()[0].active().code_address, addr(0x101));
    assert_eq!(analysis.states()[0].active().address, addr(0x200));
    assert!(analysis.outcomes().iter().any(|o| {
        o.kind == OutcomeKind::Return
            && o.store
                .read(
                    addr(0x200),
                    &AbstractValue::constant(U256::ZERO),
                    Domain::default(),
                )
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
    let first = format!("{}00", call(0xf1, 0x200, 0, 0, 0));
    let second = format!("{}00", call(0xf1, 0x300, 0, 0, 0));
    let depth = analyze_world(
        world(&[(0x101, &first), (0x200, &second), (0x300, "00")]),
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
    assert_eq!(memory.status(), Status::Converged);
    assert!(memory.frontiers().is_empty());
    assert!(
        memory
            .outcomes()
            .iter()
            .any(|o| o.kind == OutcomeKind::Return)
    );
    assert!(
        memory
            .outcomes()
            .iter()
            .any(|o| o.kind == OutcomeKind::Failure)
    );
}
