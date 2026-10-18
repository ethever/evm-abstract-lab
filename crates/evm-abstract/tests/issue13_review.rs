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
        symbolic::{ExprId, ExprKind, SlotStateId, SlotStateKind},
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

// Test-only route extraction from actual saved counter roots. A status now has
// precise numeric value 0 or 1 at its immediate continuation; the literal from
// another node cannot qualify that call's history in a later joined payload.
fn counter_route(
    root: &SlotStateId,
    target: &SlotStateId,
    limits: evm_abstract::domain::symbolic::ExprLimits,
) -> ExprId {
    fn visit(
        root: &SlotStateId,
        target: &SlotStateId,
        limits: evm_abstract::domain::symbolic::ExprLimits,
        memo: &mut std::collections::BTreeMap<SlotStateId, ExprId>,
    ) -> ExprId {
        if root == target {
            return ExprId::constant(U256::from(1));
        }
        if let Some(predicate) = memo.get(root) {
            return predicate.clone();
        }
        let predicate = match root.kind() {
            SlotStateKind::Store {
                previous,
                key,
                value,
            } => {
                // An overwrite can canonicalize away the old slot spine. The
                // actual ADD still retains its old counter read, so qualifying
                // that operand as one identifies the earlier successful write.
                if let (
                    SlotStateKind::Store {
                        key: target_key,
                        value: target_value,
                        ..
                    },
                    ExprKind::Operation { opcode: op, args },
                ) = (target.kind(), value.kind())
                    && key == target_key
                    && target_value.as_constant() == Some(U256::from(1))
                    && *op == opcode::ADD
                    && args[0].as_constant() == Some(U256::from(1))
                {
                    ExprId::operation(
                        opcode::EQ,
                        &[args[1].clone(), ExprId::constant(U256::from(1))],
                        limits,
                    )
                    .unwrap()
                } else {
                    visit(previous, target, limits, memo)
                }
            }
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => ExprId::ite(
                condition.clone(),
                visit(then_state, target, limits, memo),
                visit(else_state, target, limits, memo),
                limits,
            )
            .unwrap(),
            SlotStateKind::Zero | SlotStateKind::Input { .. } => ExprId::constant(U256::ZERO),
        };
        memo.insert(root.clone(), predicate.clone());
        predicate
    }
    assert!(root.nodes() <= limits.max_nodes && root.depth() <= limits.max_depth);
    visit(root, target, limits, &mut std::collections::BTreeMap::new())
}

#[test]
fn shared_implementation_transient_storage_is_isolated_between_proxies() {
    use evm_abstract::domain::relational::RelationState;

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
        // depth 64 and 128 atoms, with bounded local propagation work.
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
        assert_eq!(continuations.len(), 6);
        let root_bytes = alloy_primitives::hex::decode(&root).unwrap();
        for state in &continuations {
            let block = state.entry.active().key.basic_block_index;
            let pc = [15, 35, 56][block - 1];
            assert_eq!(state.active().code_address, address(0x101));
            assert_eq!(state.executed_pcs.first(), Some(&pc));
            assert_eq!(root_bytes[pc - 1], opcode::CALL);
            assert_eq!(root_bytes[pc], opcode::POP);
            assert!(state.execution_evidence().unwrap().is_current());
            // The value is the actual resumed CALL result, before POP. Current
            // child receipts, rather than old accumulated edges, prove its origin.
            let status = state.entry.active().stack.last().unwrap();
            let success = status.singleton().unwrap();
            assert!(success <= U256::from(1));
            let expected_edge = if success == U256::from(1) {
                MachineEdgeKind::Return
            } else {
                MachineEdgeKind::Failure
            };
            assert!(graph.states().iter().any(|predecessor| {
                predecessor.execution_evidence().is_some_and(|receipt| {
                    receipt.is_current()
                        && receipt.successors().contains(&(state.id, expected_edge))
                })
            }));
            assert_eq!(status.numeric(), AbstractValue::constant(success).numeric());
            assert_eq!(status.provenance(), &Provenance::constant());
            assert!(!status.symbolic_limit_reached());
        }
        let first_rejected = continuations
            .iter()
            .find(|state| {
                state.active().basic_block_index == 1
                    && state.entry.active().stack.last().unwrap().singleton() == Some(U256::ZERO)
            })
            .unwrap();
        let rejected_zero = first_rejected
            .exit
            .as_ref()
            .unwrap()
            .store
            .read_checked(address(0x101), &zero, domain)
            .unwrap();
        assert_eq!(rejected_zero.numeric(), zero.numeric());
        assert!(!rejected_zero.symbolic_limit_reached());
        let memory_source = continuations
            .iter()
            .flat_map(|state| {
                state
                    .execution_evidence()
                    .unwrap()
                    .instruction_values()
                    .unwrap()
                    .iter()
                    .filter(|value| {
                        root_bytes[state.executed_pcs[value.instruction_index()]] == opcode::MLOAD
                    })
                    .map(|value| {
                        assert_eq!(value.provenance(), &Provenance::source(Origin::Memory));
                        value.provenance().clone()
                    })
            })
            .reduce(|left, right| left.join(&right))
            .unwrap();
        // The first rejected CALL writes zero, preserving the initial zero
        // slot root. Its later join with the first successful memory load adds
        // Constant to slot0's source. All subsequent loads still include Memory.
        let initial_zero = graph.states()[0]
            .entry
            .store
            .read_checked(address(0x101), &zero, domain)
            .unwrap();
        assert_eq!(initial_zero, zero);
        let historical_source = initial_zero.provenance().join(&memory_source);
        assert_eq!(rejected_zero.provenance(), &historical_source);
        let continuations: Vec<_> = continuations
            .into_iter()
            .filter(|state| {
                state.entry.active().stack.last().unwrap().singleton() == Some(U256::from(1))
            })
            .collect();
        assert_eq!(continuations.len(), 3);
        for (index, state) in continuations.iter().enumerate() {
            assert_eq!(state.active().basic_block_index, index + 1);
        }
        let all_returns: Vec<_> = graph
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .collect();
        // Splitting the last call also improves its failed continuation: it can
        // retain only the previous 0/1 count, never the successful increment 2.
        for success in [false, true] {
            let returns: Vec<_> = all_returns
                .iter()
                .filter(|outcome| {
                    graph.states()[outcome.state]
                        .entry
                        .active()
                        .stack
                        .last()
                        .unwrap()
                        .singleton()
                        == Some(U256::from(u8::from(success)))
                })
                .collect();
            assert!(!returns.is_empty());
            for outcome in returns {
                let payload = graph.states()[outcome.state].exit.as_ref().unwrap();
                let maximum = if success { 2 } else { 1 };
                for (slot, bound) in [(0, 1), (1, 1), (2, maximum)] {
                    assert_eq!(
                        outcome
                            .store
                            .read_checked(
                                address(0x101),
                                &AbstractValue::constant(U256::from(slot)),
                                domain
                            )
                            .unwrap()
                            .numeric(),
                        union(bound).numeric()
                    );
                }
                for (owner, bound) in [(0x201, maximum), (0x202, 1), (0x300, 0), (0x101, 0)] {
                    assert_eq!(
                        payload
                            .store
                            .read_transient_checked(address(owner), &zero, domain)
                            .unwrap()
                            .numeric(),
                        union(bound).numeric()
                    );
                }
            }
        }
        let returns: Vec<_> = all_returns
            .into_iter()
            .filter(|outcome| {
                graph.states()[outcome.state]
                    .entry
                    .active()
                    .stack
                    .last()
                    .unwrap()
                    .singleton()
                    == Some(U256::from(1))
            })
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
                let origins = read
                    .provenance()
                    .origins()
                    .sources()
                    .expect("persistent read lost its finite origins");
                let required = if slot == 0 {
                    &historical_source
                } else {
                    &memory_source
                };
                assert!(required.origins().sources().unwrap().is_subset(origins));
                // Literal operand normalization adds Constant; the slot reader
                // unions root and pointwise-cache may-origins without changing
                // numeric facts. Neither source can introduce another origin.
                assert!(
                    origins.is_subset(&std::collections::BTreeSet::from([
                        Origin::Constant,
                        Origin::Memory,
                    ])),
                    "slot {slot} gained an unrelated origin"
                );
                assert!(!read.provenance().is_code_address());
                // These literal counters and MLOAD results carry no immutable
                // input identity or live block definition. Identity equality
                // ignores runtime aliases, so check same_identity as well.
                assert!(read.identity().input().is_none());
                assert!(!read.same_identity(&read));
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
            let initial_root = graph.states()[0]
                .entry
                .store
                .slot_root(address(0x101), false)
                .unwrap();
            assert!(persistent_root.nodes() <= limits.max_nodes);
            assert!(persistent_root.depth() <= limits.max_depth);
            // Separate CALL results retain real historical ITE roots. Every
            // branch must still contain descending writes to 2, 1, 0 and end
            // at the exact initial zero root; canonical zero writes can vanish.
            for (slot, read) in persistent.iter().enumerate() {
                let key = ExprId::constant(U256::from(slot));
                let expression =
                    ExprId::slot_read(persistent_root.clone(), key.clone(), limits.expressions())
                        .unwrap_or_else(|error| {
                            panic!(
                                "{fork:?} state{} slot {slot}: root read: {error:?}",
                                outcome.state
                            )
                        });
                assert!(read.expression() == Some(&expression));
                let prove_equal = |qualified: &RelationState,
                                   left: &ExprId,
                                   right: &ExprId,
                                   leaf: &SlotStateId,
                                   obligation: &str| {
                    let equality = ExprId::operation(
                        opcode::EQ,
                        &[left.clone(), right.clone()],
                        limits.expressions(),
                    )
                    .unwrap_or_else(|error| {
                        panic!(
                            "{fork:?} state{} slot {slot} leaf{} {obligation}: EQ construction: {error:?}",
                            outcome.state, leaf.id()
                        )
                    });
                    // Refute only the counterexample; never assume the desired
                    // equality or import an optional operand guarantee into F.
                    let mut counterexample = qualified.clone();
                    let checked = counterexample
                        .assume_checked(&equality, false, &limits)
                        .unwrap_or_else(|reason| {
                            panic!(
                                "{fork:?} state{} slot {slot} leaf{} {obligation}: disequality retention: {reason:?}",
                                outcome.state, leaf.id()
                            )
                        });
                    assert_eq!(
                        checked,
                        CheckResult::ProvenImpossible,
                        "{fork:?} state{} slot {slot} root{} leaf{} {obligation}; EQ nodes={} depth={} atoms={} work={}",
                        outcome.state,
                        persistent_root.id(),
                        leaf.id(),
                        equality.nodes(),
                        equality.depth(),
                        qualified.len(),
                        qualified.work_size()
                    );
                };
                // A1's common must-table is not a historical value archive.
                // Independently pair each leaf with the actual complete root
                // under the payload guarantees and its entire selector path.
                let mut pending = vec![(&persistent_root, 3, payload.relations.clone())];
                let mut visited = 0;
                let mut leaves = 0;
                while let Some((state, upper, qualified)) = pending.pop() {
                    // Count expanded visits: equal nodes reached by different
                    // selector paths must never discard each other's guards.
                    visited += 1;
                    assert!(visited <= limits.max_nodes, "persistent path-visit limit");
                    let value = match state.kind() {
                        SlotStateKind::Store {
                            previous,
                            key,
                            value,
                        } => {
                            let written: usize = key.as_constant().unwrap().try_into().unwrap();
                            assert!(written < upper, "out-of-order persistent write");
                            if written == 0 {
                                assert_eq!(previous, &initial_root);
                            }
                            if written == slot {
                                value.clone()
                            } else {
                                pending.push((previous, written, qualified));
                                continue;
                            }
                        }
                        SlotStateKind::Ite {
                            condition,
                            then_state,
                            else_state,
                        } => {
                            for (next, nonzero) in [(then_state, true), (else_state, false)] {
                                let mut branch = qualified.clone();
                                branch
                                    .assume_checked(condition, nonzero, &limits)
                                    .unwrap_or_else(|reason| {
                                        panic!(
                                            "{fork:?} state{} slot {slot} ITE{} guard={nonzero}: retention: {reason:?}",
                                            outcome.state, state.id()
                                        )
                                    });
                                // Even Undetermined retains the guard. Keep all
                                // branches, including structurally impossible ones.
                                pending.push((next, upper, branch));
                            }
                            continue;
                        }
                        SlotStateKind::Zero => {
                            assert_eq!(state, &initial_root);
                            ExprId::constant(U256::ZERO)
                        }
                        SlotStateKind::Input { .. } => panic!("lost initial persistent root"),
                    };
                    leaves += 1;
                    prove_equal(&qualified, &expression, &value, state, "root/leaf value");
                    for term in [&key, &value] {
                        if let Some((_, guarantee)) = payload
                            .store
                            .symbolic_facts(false)
                            .find(|(candidate, _)| *candidate == term)
                        {
                            assert!(!guarantee.symbolic_limit_reached());
                            assert!(guarantee.identity().input().is_none());
                            assert!(!guarantee.same_identity(guarantee));
                            assert!(!guarantee.provenance().is_code_address());
                            assert!(
                                guarantee
                                    .provenance()
                                    .origins()
                                    .sources()
                                    .unwrap()
                                    .is_subset(&std::collections::BTreeSet::from([
                                        Origin::Constant,
                                        Origin::Memory,
                                    ]))
                            );
                            // refine_operands may compact away this expression;
                            // the table key remains authoritative when absent.
                            if let Some(retained) = guarantee.expression() {
                                prove_equal(
                                    &qualified,
                                    term,
                                    retained,
                                    state,
                                    "optional operand expression",
                                );
                            }
                            if let Some(constant) = term.as_constant() {
                                assert!(guarantee.contains(constant));
                            } else if !qualified.is_bottom() {
                                // The independently proved root/leaf equality
                                // bounds this term by the actual finite read.
                                // Require coverage of every candidate that the
                                // qualified query cannot exclude, not a model.
                                let mut candidates = 0;
                                for candidate in read.constants().unwrap() {
                                    match qualified.can_equal(term, *candidate, &limits) {
                                        CheckResult::ProvenImpossible => {}
                                        CheckResult::MayBeFeasible => {
                                            candidates += 1;
                                            assert!(
                                                guarantee.contains(*candidate),
                                                "{fork:?} state{} slot {slot} leaf{}: optional operand excludes {candidate}",
                                                outcome.state,
                                                state.id()
                                            );
                                        }
                                        CheckResult::Undetermined(reason) => panic!(
                                            "{fork:?} state{} slot {slot} leaf{}: optional operand numeric query: {reason:?}",
                                            outcome.state,
                                            state.id()
                                        ),
                                    }
                                }
                                assert!(
                                    candidates > 0,
                                    "optional operand lost all read candidates"
                                );
                            }
                        }
                    }
                }
                assert!(leaves > 0, "missing persistent root/leaf witness");
            }

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
            // increment must use its actual callee input. Every current block
            // executing for owner202 must leave owner201's root unchanged.
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
            for state in graph
                .states()
                .iter()
                .filter(|state| state.active().address == address(0x202))
            {
                let exit = state.exit.as_ref().unwrap();
                assert_eq!(
                    state.entry.store.slot_root(address(0x201), true).unwrap(),
                    exit.store.slot_root(address(0x201), true).unwrap()
                );
            }
            let third_implementation = graph
                .states()
                .iter()
                .find(|state| {
                    state.key.frames.len() == 3
                        && state.key.frames[0].basic_block_index == 2
                        && state.active().address == address(0x201)
                        && state.active().code_address == address(0x300)
                        && state.active().basic_block_index == 0
                })
                .unwrap();
            let before_third = &third_implementation.entry.store;
            let old_count = before_third
                .read_transient_checked(address(0x201), &zero, domain)
                .unwrap();
            assert_eq!(old_count.numeric(), union(1).numeric());
            assert_eq!(old_count.provenance(), &counter_source);
            assert_eq!(
                writes(&before_third.slot_root(address(0x201), true).unwrap()),
                std::collections::BTreeSet::from([ExprId::constant(U256::from(1))])
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
            let mut pending = vec![before_third.slot_root(address(0x201), true).unwrap()];
            let mut actual_inputs = std::collections::BTreeSet::new();
            let mut increments = std::collections::BTreeSet::new();
            while let Some(root) = pending.pop() {
                if !actual_inputs.insert(root.clone()) {
                    continue;
                }
                assert!(actual_inputs.len() <= limits.max_nodes);
                increments.insert(
                    ExprId::operation(
                        opcode::ADD,
                        &[
                            ExprId::constant(U256::from(1)),
                            ExprId::slot_read(
                                root.clone(),
                                ExprId::constant(U256::ZERO),
                                limits.expressions(),
                            )
                            .unwrap(),
                        ],
                        limits.expressions(),
                    )
                    .unwrap(),
                );
                match root.kind() {
                    SlotStateKind::Store { previous, .. } => pending.push(previous.clone()),
                    SlotStateKind::Ite {
                        then_state,
                        else_state,
                        ..
                    } => pending.extend([then_state.clone(), else_state.clone()]),
                    SlotStateKind::Zero => {}
                    SlotStateKind::Input { .. } => panic!("lost actual increment input"),
                }
            }
            let actual_writes = writes(&histories[0]);
            assert!(actual_writes.contains(&ExprId::constant(U256::from(1))));
            assert!(actual_writes.contains(&increment));
            // Outputs retain earlier executions of the same joined entry.
            // Each retained increment must read an actual owner201 input root,
            // including its historical ITE branches, and add exactly one.
            assert!(actual_writes.is_subset(&increments));
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
                // MayBeFeasible only means no local contradiction was proved.
                // The concrete outcomes and exact controls above remain separate checks.
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
                CheckResult::MayBeFeasible,
            );
            // Returned values must determine owner201's paired counter even
            // without selecting an increment history through counter_route.
            check(
                "returned values independently determine owner201 counter",
                &and(persistent_premise.clone(), not(equal(&counters[0], 2))),
                CheckResult::ProvenImpossible,
            );

            let terminal_inputs = persistent[2].expression().unwrap();
            assert!(
                terminal_inputs.nodes() <= limits.max_nodes
                    && terminal_inputs.depth() <= limits.max_depth
            );
            let mut terminal_bindings = terminal_inputs.variables();
            for value in persistent.iter().chain(&counters) {
                terminal_bindings.extend(value.symbolic_expression().unwrap().variables());
            }
            let routes = [0x201, 0x202].map(|owner| {
                let index = usize::from(owner == 0x202);
                let implementation = graph
                    .states()
                    .iter()
                    .find(|state| {
                        state.key.frames.len() == 3
                            && state.key.frames[0].basic_block_index == index
                            && state.active().address == address(owner)
                            && state.active().code_address == address(0x300)
                            && state.active().basic_block_index == 0
                    })
                    .unwrap();
                assert_eq!(
                    implementation
                        .entry
                        .store
                        .read_transient_checked(address(owner), &zero, domain)
                        .unwrap()
                        .numeric(),
                    zero.numeric()
                );
                // A successful proxy CALL can contain a rejected DELEGATECALL.
                // Qualify the actual implementation TSTORE's successful root,
                // rather than the proxy's joined success-continuation root.
                let saved = implementation
                    .exit
                    .as_ref()
                    .unwrap()
                    .store
                    .slot_root(address(owner), true)
                    .unwrap();
                let route = counter_route(&histories[index], &saved, limits.expressions());
                assert!(
                    !route.variables().is_empty(),
                    "historical CALL route was not retained"
                );
                assert!(route.variables().is_subset(&terminal_bindings));
                route
            });
            let success = routes
                .iter()
                .cloned()
                .fold(persistent_premise.clone(), &and);
            check(
                "successful call guards remain locally admissible",
                &success,
                CheckResult::MayBeFeasible,
            );
            check(
                "successful guards do not locally exclude expected counters",
                &and(success.clone(), joint),
                CheckResult::MayBeFeasible,
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
                CheckResult::ProvenImpossible,
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
                CheckResult::ProvenImpossible,
            );

            // Persistent slot1=1 can retain the first CALL's output after the
            // second CALL is rejected. Preserve that legitimate guarded case.
            let rejected_second = and(
                and(
                    persistent_premise,
                    and(routes[0].clone(), not(routes[1].clone())),
                ),
                equal(&counters[1], 0),
            );
            check(
                "second call rejected with old memory retained",
                &rejected_second,
                CheckResult::MayBeFeasible,
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
