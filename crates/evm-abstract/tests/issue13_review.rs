//! Independent issue-13 acceptance regressions across call/store/SSA boundaries.
#[path = "summaries/oracle.rs"]
mod oracle;

use evm_abstract::{
    Fork, U256,
    analysis::{ExecutionConfig, MachineEdgeKind, OutcomeKind, Status, analyze_world},
    domain::{
        AbstractValue, Domain,
        provenance::{Origin, OriginSet, Provenance},
        relational::CheckResult,
        symbolic::{ExprId, SlotStateId, SlotStateKind},
    },
    ssa,
    world::{Account, ByteArray, Entry, World},
};
use oracle::address;
use revm_bytecode::opcode;

fn entry() -> Entry {
    Entry {
        address: address(0x101),
        environment: evm_abstract::world::EvmEnvironment {
            to: (address(0x101)).into(),
            caller: (address(0x1000)).into(),
            value: AbstractValue::constant(U256::ZERO),
            calldata: ByteArray::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironment::default()
        },
    }
}

fn call(target: u16, value: u8) -> String {
    format!("60205f5f5f60{value:02x}61{target:04x}620f4240f1")
}

#[test]
fn shared_implementation_transient_storage_is_isolated_between_proxies() {
    let root = format!(
        "{}505f515f55{}505f51600155{}505f5160025500",
        call(0x201, 0),
        call(0x202, 0),
        call(0x201, 0)
    );
    let proxy = "60205f5f5f610300620f4240f45060205ff3";
    let implementation = "5f5c600101805f5d5f5260205ff3";
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let mut world = World::new(fork, "review:two-proxy-transient");
        for (number, code) in [
            (0x101, root.as_str()),
            (0x201, proxy),
            (0x202, proxy),
            (0x300, implementation),
        ] {
            world
                .insert(address(number), Account::from_hex(code, fork).unwrap())
                .unwrap();
        }
        let graph = analyze_world(world.clone(), entry(), ExecutionConfig::default()).unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert_eq!(
            oracle::compare(&world, &entry(), &graph),
            OutcomeKind::Return
        );
        ssa::build_world(&graph).unwrap().verify(&graph).unwrap();
        let zero = AbstractValue::constant(U256::ZERO);
        let domain = Domain::default();
        for outcome in graph.outcomes() {
            assert!(outcome.store.transient_slots().is_empty());
            for owner in [0x101, 0x201, 0x202, 0x300] {
                assert_eq!(
                    outcome
                        .store
                        .read_transient_checked(address(owner), &zero, domain)
                        .unwrap(),
                    zero
                );
                assert!(matches!(
                    outcome
                        .store
                        .slot_root(address(owner), true)
                        .unwrap()
                        .kind(),
                    evm_abstract::domain::symbolic::SlotStateKind::Zero
                ));
            }
        }
        // Inspection uses the unchanged analysis query policy: 1024 nodes,
        // depth 64, 128 atoms and the finite default provider resource limit.
        let limits = domain.spec().relations();
        assert_eq!(graph.domain_spec().relations(), limits);
        let union = |maximum: u64| {
            (1..=maximum).fold(zero.clone(), |value, n| {
                domain.join(&value, &AbstractValue::constant(U256::from(n)))
            })
        };
        let counter_source = Provenance::from_origins(
            OriginSet::from_sources([
                Origin::Constant,
                Origin::TransientStorage,
                Origin::Arithmetic,
            ])
            .unwrap(),
        );
        let mut continuations: Vec<_> = graph
            .states()
            .iter()
            .filter(|state| {
                state.key.frames.len() == 1
                    && state.active().address == address(0x101)
                    && state.entry.active().key.basic_block_index != 0
            })
            .collect();
        continuations.sort_by_key(|state| state.entry.active().key.basic_block_index);
        assert_eq!(continuations.len(), 3);
        let root_bytes = alloy_primitives::hex::decode(&root).unwrap();
        for (index, (state, pc)) in continuations.iter().zip([15, 35, 56]).enumerate() {
            assert_eq!(state.entry.active().key.basic_block_index, index + 1);
            assert_eq!(state.active().code_address, address(0x101));
            assert_eq!(state.executed_pcs.first(), Some(&pc));
            assert_eq!(root_bytes[pc - 1], opcode::CALL);
            assert_eq!(root_bytes[pc], opcode::POP);
            assert!(state.execution_evidence().unwrap().is_current());
            // The value is the actual resumed CALL result, before POP. Current
            // child receipts, rather than old accumulated edges, prove its origin.
            assert!(graph.states().iter().any(|predecessor| {
                predecessor.execution_evidence().is_some_and(|receipt| {
                    receipt.is_current()
                        && receipt
                            .successors()
                            .contains(&(state.id, MachineEdgeKind::Return))
                })
            }));
            let status = state.entry.active().stack.last().unwrap();
            assert_eq!(status.numeric(), union(1).numeric());
            assert_eq!(status.provenance(), &Provenance::constant());
            assert!(status.expression().is_some());
            assert!(!status.symbolic_limit_reached());
        }
        let returns: Vec<_> = graph
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .collect();
        assert!(!returns.is_empty());
        for outcome in returns {
            // STOP's current exit precedes transaction finalization and retains
            // the three calls' paired account-local counter histories.
            let terminal = &graph.states()[outcome.state];
            assert_eq!(terminal.key.frames.len(), 1);
            assert_eq!(terminal.active().address, address(0x101));
            assert_eq!(terminal.active().code_address, address(0x101));
            assert_eq!(terminal.executed_pcs.last(), Some(&(root.len() / 2 - 1)));
            assert_eq!(root_bytes[root_bytes.len() - 1], opcode::STOP);
            let receipt = terminal.execution_evidence().unwrap();
            assert!(receipt.is_current());
            assert_eq!(receipt.instructions().len(), terminal.executed_pcs.len());
            assert_eq!(
                receipt.instructions().last(),
                Some(&evm_abstract::analysis::InstructionProgress::Dispatched)
            );
            let payload = terminal.exit.as_ref().unwrap();
            let persistent = [(0, 1), (1, 1), (2, 2)].map(|(slot, expected)| {
                let key = AbstractValue::constant(U256::from(slot));
                let read = outcome
                    .store
                    .read_checked(address(0x101), &key, domain)
                    .unwrap();
                // Preserve the original finite/expected checks, plus every
                // numeric component and the complete memory-source provenance.
                assert!(read.constants().is_some());
                assert!(read.contains(U256::from(expected)));
                assert_eq!(read.numeric(), union(expected).numeric());
                assert_eq!(read.provenance(), &Provenance::source(Origin::Memory));
                assert!(!read.symbolic_limit_reached());
                assert_eq!(
                    read,
                    payload
                        .store
                        .read_checked(address(0x101), &key, domain)
                        .unwrap()
                );
                read
            });
            let persistent_root = payload.store.slot_root(address(0x101), false).unwrap();
            assert_eq!(
                persistent_root,
                outcome.store.slot_root(address(0x101), false).unwrap()
            );
            let mut previous = &persistent_root;
            for (slot, read) in persistent.iter().enumerate().rev() {
                let SlotStateKind::Store {
                    previous: older,
                    key,
                    value,
                } = previous.kind()
                else {
                    panic!("missing ordered persistent write at slot {slot}");
                };
                assert_eq!(key.as_constant(), Some(U256::from(slot)));
                assert_eq!(Some(value), read.expression());
                let (_, source) = payload
                    .store
                    .symbolic_facts(false)
                    .find(|(term, _)| *term == value)
                    .unwrap();
                assert_eq!(read.numeric(), source.numeric());
                assert_eq!(read.provenance(), source.provenance());
                assert_eq!(read.identity(), source.identity());
                assert_eq!(read.expression(), source.expression());
                previous = older;
            }
            assert!(matches!(previous.kind(), SlotStateKind::Zero));
            assert_eq!(
                previous,
                &graph.states()[0]
                    .entry
                    .store
                    .slot_root(address(0x101), false)
                    .unwrap()
            );

            let owners = [0x201, 0x202, 0x300, 0x101];
            let counters = owners.map(|owner| {
                payload
                    .store
                    .read_transient_checked(address(owner), &zero, domain)
                    .unwrap()
            });
            let histories =
                owners.map(|owner| payload.store.slot_root(address(owner), true).unwrap());
            for (index, expected) in [2, 1, 0, 0].into_iter().enumerate() {
                assert_eq!(counters[index].numeric(), union(expected).numeric());
                assert!(!counters[index].symbolic_limit_reached());
                if index < 2 {
                    assert_eq!(counters[index].provenance(), &counter_source);
                    assert!(counters[index].expression().is_some());
                    assert!(matches!(histories[index].kind(), SlotStateKind::Ite { .. }));
                } else {
                    assert_eq!(counters[index], zero);
                    assert!(matches!(histories[index].kind(), SlotStateKind::Zero));
                }
                assert!(matches!(
                    graph.states()[0]
                        .entry
                        .store
                        .slot_root(address(owners[index]), true)
                        .unwrap()
                        .kind(),
                    SlotStateKind::Zero
                ));
            }
            // Traverse only the bounded immutable slot spine. The second 201
            // increment must use its old actual read; 202 cannot change that root.
            let writes = |root: &SlotStateId| {
                assert!(root.nodes() <= limits.max_nodes && root.depth() <= limits.max_depth);
                let mut pending = vec![root];
                let mut values = std::collections::BTreeSet::new();
                let mut visited = 0;
                while let Some(state) = pending.pop() {
                    visited += 1;
                    assert!(visited <= limits.max_nodes);
                    match state.kind() {
                        SlotStateKind::Zero => {}
                        SlotStateKind::Store {
                            previous,
                            key,
                            value,
                        } => {
                            assert_eq!(key.as_constant(), Some(U256::ZERO));
                            values.insert(value.clone());
                            pending.push(previous);
                        }
                        SlotStateKind::Ite {
                            then_state,
                            else_state,
                            ..
                        } => {
                            pending.extend([then_state, else_state]);
                        }
                        SlotStateKind::Input { .. } => {
                            panic!("lost complete initial counter source")
                        }
                    }
                }
                values
            };
            let after_first = &continuations[0].entry.store;
            let before_third = &continuations[1].entry.store;
            let old_count = before_third
                .read_transient_checked(address(0x201), &zero, domain)
                .unwrap();
            assert_eq!(
                old_count,
                after_first
                    .read_transient_checked(address(0x201), &zero, domain)
                    .unwrap()
            );
            assert_eq!(
                before_third.slot_root(address(0x201), true).unwrap(),
                after_first.slot_root(address(0x201), true).unwrap()
            );
            let increment = ExprId::operation(
                opcode::ADD,
                &[
                    ExprId::constant(U256::from(1)),
                    old_count.symbolic_expression().unwrap(),
                ],
                limits.expressions(),
            )
            .unwrap();
            assert_eq!(
                writes(&histories[0]),
                std::collections::BTreeSet::from([ExprId::constant(U256::from(1)), increment])
            );
            assert_eq!(
                writes(&histories[1]),
                std::collections::BTreeSet::from([ExprId::constant(U256::from(1))])
            );

            let equal = |value: &AbstractValue, expected| {
                ExprId::operation(
                    opcode::EQ,
                    &[
                        value.symbolic_expression().unwrap(),
                        ExprId::constant(U256::from(expected)),
                    ],
                    limits.expressions(),
                )
                .unwrap()
            };
            let and = |left, right| {
                ExprId::operation(opcode::AND, &[left, right], limits.expressions()).unwrap()
            };
            let not =
                |value| ExprId::operation(opcode::ISZERO, &[value], limits.expressions()).unwrap();
            let check = |name, predicate: &ExprId, expected| {
                let actual = payload
                    .relations
                    .can_equal(predicate, U256::from(1), &limits);
                eprintln!(
                    "{fork:?} state{} {name}: {actual:?}; nodes={} depth={}",
                    outcome.state,
                    predicate.nodes(),
                    predicate.depth()
                );
                // UNKNOWN and solver errors are neither SAT witnesses nor proofs.
                assert_eq!(actual, expected, "{fork:?} {name}");
            };
            let persistent_premise = persistent
                .iter()
                .zip([1, 1, 2])
                .fold(ExprId::constant(U256::from(1)), |p, (v, n)| {
                    and(p, equal(v, n))
                });
            let joint = counters
                .iter()
                .zip([2, 1, 0, 0])
                .fold(persistent_premise.clone(), |p, (v, n)| and(p, equal(v, n)));
            check(
                "joint expected storage and counters",
                &joint,
                CheckResult::Sat,
            );

            let terminal_inputs = persistent[2].expression().unwrap();
            assert!(
                terminal_inputs.nodes() <= limits.max_nodes
                    && terminal_inputs.depth() <= limits.max_depth
            );
            let terminal_bindings = terminal_inputs.variables();
            let success = continuations
                .iter()
                .fold(persistent_premise.clone(), |p, state| {
                    let status = state.entry.active().stack.last().unwrap();
                    let expression = status.expression().unwrap();
                    assert!(
                        expression.nodes() <= limits.max_nodes
                            && expression.depth() <= limits.max_depth
                    );
                    let bindings = expression.variables();
                    assert!(!bindings.is_empty());
                    assert!(bindings.is_subset(&terminal_bindings));
                    and(p, equal(status, 1))
                });
            check(
                "all three actual calls succeeded",
                &success,
                CheckResult::Sat,
            );
            check(
                "same successful model has all expected counters",
                &and(success.clone(), joint),
                CheckResult::Sat,
            );
            let wrong = counters.iter().zip([2, 1, 0, 0]).fold(
                ExprId::constant(U256::ZERO),
                |p, (v, n)| {
                    ExprId::operation(opcode::OR, &[p, not(equal(v, n))], limits.expressions())
                        .unwrap()
                },
            );
            check(
                "qualified wrong owner counter",
                &and(success, wrong),
                CheckResult::Unsat,
            );
            let unused_owner_wrong = ExprId::operation(
                opcode::OR,
                &[not(equal(&counters[2], 0)), not(equal(&counters[3], 0))],
                limits.expressions(),
            )
            .unwrap();
            check(
                "code/caller owner counter contamination",
                &unused_owner_wrong,
                CheckResult::Unsat,
            );

            // Persistent slot1=1 can retain the first CALL's output after the
            // second CALL is rejected. Preserve that legitimate guarded model.
            let second_status = continuations[1].entry.active().stack.last().unwrap();
            let rejected_second = and(
                and(persistent_premise, equal(second_status, 0)),
                equal(&counters[1], 0),
            );
            check(
                "second call rejected with old memory retained",
                &rejected_second,
                CheckResult::Sat,
            );
            for (index, owner) in owners.into_iter().enumerate() {
                assert_eq!(
                    payload.store.slot_root(address(owner), true).unwrap(),
                    histories[index]
                );
                assert_eq!(
                    payload
                        .store
                        .read_transient_checked(address(owner), &zero, domain)
                        .unwrap(),
                    counters[index]
                );
            }
        }
    }
}

#[test]
fn value_call_to_confirmed_empty_account_creates_present_state() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let root = format!("{}506102003f5f5260205ff3", call(0x200, 5));
        let mut world = World::new(fork, "review:empty-value-call");
        let mut root_account = Account::from_hex(&root, fork).unwrap();
        root_account.balance = AbstractValue::constant(U256::from(100));
        world.insert(address(0x101), root_account).unwrap();
        world.insert(address(0x200), Account::absent()).unwrap();
        let graph = analyze_world(world.clone(), entry(), ExecutionConfig::default()).unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert_eq!(
            oracle::compare(&world, &entry(), &graph),
            OutcomeKind::Return
        );
        ssa::build_world(&graph).unwrap().verify(&graph).unwrap();
        assert!(
            graph
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == OutcomeKind::Return
                    && outcome.store.read_balance(address(0x200)).singleton()
                        == AbstractValue::constant(U256::from(5)).singleton()
                    && outcome.store.existence(address(0x200))
                        == evm_abstract::world::Existence::Present
                    && outcome
                        .data
                        .read_word(&AbstractValue::constant(U256::ZERO), Domain::default())
                        .contains(U256::from_be_slice(
                            alloy_primitives::keccak256([]).as_slice()
                        ))),
            "value transfer retained absent-account metadata: {:?}",
            graph.outcomes()
        );
    }
}

#[test]
fn balance_updates_preserve_sound_account_presence_for_mixed_values() {
    let mut world = World::new(Fork::Osaka, "review:balance-presence");
    world.insert(address(0x200), Account::absent()).unwrap();
    world.insert(address(0x201), Account::empty()).unwrap();
    let mut exact = evm_abstract::world::Store::new(&world);
    exact.write_balance(address(0x200), AbstractValue::constant(U256::from(5)));
    assert_eq!(
        exact.existence(address(0x200)),
        evm_abstract::world::Existence::Present
    );
    let mut mixed = evm_abstract::world::Store::new(&world);
    mixed.write_balance(
        address(0x200),
        Domain::default().join(
            &AbstractValue::constant(U256::ZERO),
            &AbstractValue::constant(U256::from(5)),
        ),
    );
    assert_eq!(
        mixed.existence(address(0x200)),
        evm_abstract::world::Existence::Unknown
    );
    mixed.write_balance(address(0x201), AbstractValue::constant(U256::ZERO));
    assert_eq!(
        mixed.existence(address(0x201)),
        evm_abstract::world::Existence::Present
    );
}

#[test]
fn prague_and_osaka_bls_native_operations_cover_valid_and_invalid_inputs() {
    // Infinity uses zero point coordinates. Mapping the zero field element is
    // also a valid nontrivial native operation; the independent oracle checks
    // the actual output rather than importing expected bytes from our transfer.
    for fork in [Fork::Prague, Fork::Osaka] {
        for (number, input_size, output_size) in [
            (0x0b, 256, 128),
            (0x0c, 160, 128),
            (0x0d, 512, 256),
            (0x0e, 288, 256),
            (0x0f, 384, 32),
            (0x10, 64, 128),
            (0x11, 128, 256),
        ] {
            let world = World::new(fork, "review:bls-native");
            let mut input = entry();
            input.address = address(number);
            input.environment.to = address(number).into();
            input.environment.calldata = ByteArray::exact(&vec![0; input_size]);
            let graph = analyze_world(
                world.clone(),
                input.clone(),
                ExecutionConfig {
                    max_work: 10_000_000,
                    ..ExecutionConfig::default()
                },
            )
            .unwrap();
            assert_eq!(
                graph.status(),
                Status::Converged,
                "{fork} native{number:x}: {:?}",
                graph.frontiers()
            );
            assert!(
                graph
                    .outcomes()
                    .iter()
                    .any(|outcome| outcome.kind == OutcomeKind::Return
                        && outcome.data.len() == &AbstractValue::constant(U256::from(output_size)))
            );
            assert_eq!(oracle::compare(&world, &input, &graph), OutcomeKind::Return);
            ssa::build_world(&graph).unwrap().verify(&graph).unwrap();
            input.environment.calldata = ByteArray::empty();
            let invalid =
                analyze_world(world.clone(), input.clone(), ExecutionConfig::default()).unwrap();
            assert_eq!(invalid.status(), Status::Converged);
            assert!(
                !invalid
                    .outcomes()
                    .iter()
                    .any(|outcome| outcome.kind == OutcomeKind::Return)
            );
            assert_eq!(
                oracle::compare(&world, &input, &invalid),
                OutcomeKind::Failure
            );
            ssa::build_world(&invalid)
                .unwrap()
                .verify(&invalid)
                .unwrap();
        }
    }
}
