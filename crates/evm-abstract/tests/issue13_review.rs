//! Independent issue-13 acceptance regressions across call/store/SSA boundaries.
#[path = "issue13_review/joint.rs"]
mod joint;
#[path = "summaries/oracle.rs"]
mod oracle;

use evm_abstract::{
    Fork, U256,
    analysis::{ExecutionConfig, MachineEdgeKind, OutcomeKind, Status, analyze_world},
    domain::{
        AbstractValue, Domain,
        provenance::{Origin, OriginSet, Provenance},
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
    use evm_abstract::domain::{
        identity::Symbol,
        symbolic::{ExprError, ExprLimits, SourceId},
    };
    use std::collections::BTreeMap;

    // Only ITE denotation is used here. All other terms retain their complete
    // identity, including word-valued guards and captured source snapshots.
    fn specialize_ites(
        term: &ExprId,
        choices: &BTreeMap<ExprId, bool>,
        limits: ExprLimits,
    ) -> Result<ExprId, ExprError> {
        fn visit(
            term: &ExprId,
            choices: &BTreeMap<ExprId, bool>,
            limits: ExprLimits,
            original: ExprLimits,
            visits: &mut usize,
            depth: usize,
        ) -> Result<ExprId, ExprError> {
            *visits += 1;
            if *visits > original.max_nodes || depth > original.max_depth {
                return Err(ExprError::Limit {
                    nodes: *visits,
                    depth,
                });
            }
            let ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } = term.kind()
            else {
                return Ok(term.clone());
            };
            let mut descend = |value| visit(value, choices, limits, original, visits, depth + 1);
            if let Some(nonzero) = choices.get(condition) {
                descend(if *nonzero { then_value } else { else_value })
            } else {
                let condition = descend(condition)?;
                let yes = descend(then_value)?;
                let no = descend(else_value)?;
                ExprId::ite(condition, yes, no, limits)
            }
        }
        let original = ExprLimits {
            max_nodes: term.nodes(),
            max_depth: term.depth(),
        };
        if original.max_nodes > limits.max_nodes || original.max_depth > limits.max_depth {
            return Err(ExprError::Limit {
                nodes: term.nodes(),
                depth: term.depth(),
            });
        }
        let result = visit(term, choices, limits, original, &mut 0, 1)?;
        if result.nodes() > original.max_nodes || result.depth() > original.max_depth {
            return Err(ExprError::Limit {
                nodes: result.nodes(),
                depth: result.depth(),
            });
        }
        Ok(result)
    }

    // Factoring can move an old counter root into the current read's guard.
    // Only new, target-disjoint Fresh selectors may expose that exact history.
    fn increment_cofactor(current: &ExprId, target: &ExprId, limits: ExprLimits) -> bool {
        if [current, target]
            .iter()
            .any(|term| term.nodes() > limits.max_nodes || term.depth() > limits.max_depth)
        {
            return false;
        }
        let ExprKind::Operation { opcode: op, args } = target.kind() else {
            return false;
        };
        let [amount, read] = args.as_slice() else {
            return false;
        };
        if *op != opcode::ADD || amount.as_constant() != Some(U256::from(1)) {
            return false;
        }
        let (
            ExprKind::Ite {
                condition: current_guard,
                then_value: current_yes,
                else_value: current_no,
            },
            ExprKind::Ite {
                condition: target_guard,
                then_value: target_yes,
                else_value: target_no,
            },
        ) = (current.kind(), read.kind())
        else {
            return false;
        };
        if current_yes != target_yes || current_no != target_no {
            return false;
        }
        let target_fresh = target.fresh_leaves();
        let mut pending = vec![(current_guard, BTreeMap::<ExprId, bool>::new(), 1)];
        let mut visits = 0;
        while let Some((guard, choices, depth)) = pending.pop() {
            // Count paths in the original expanded tree; node-only deduplication
            // could discard a different consistent selector assignment.
            visits += 1;
            if visits > current_guard.nodes() || depth > current_guard.depth() {
                return false;
            }
            if guard == target_guard {
                let rebuilt = specialize_ites(current, &choices, limits).and_then(|read| {
                    ExprId::operation(opcode::ADD, &[amount.clone(), read], limits)
                });
                match rebuilt {
                    Ok(increment) if &increment == target => return true,
                    Ok(_) => {}
                    Err(_) => return false,
                }
            }
            let ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } = guard.kind()
            else {
                continue;
            };
            if !matches!(condition.kind(), ExprKind::Fresh(_)) || target_fresh.contains(condition) {
                continue;
            }
            for (next, nonzero) in [(else_value, false), (then_value, true)] {
                let mut branch = choices.clone();
                if let Some(previous) = branch.get(condition) {
                    if *previous != nonzero {
                        continue;
                    }
                } else {
                    branch.insert(condition.clone(), nonzero);
                }
                pending.push((next, branch, depth + 1));
            }
        }
        false
    }

    let proof_limits = Domain::default().spec().relations().expressions();
    let first_guard = ExprId::fresh().unwrap();
    let second_guard = ExprId::fresh().unwrap();
    let nested = |yes, no| {
        ExprId::ite(
            second_guard.clone(),
            ExprId::constant(U256::from(yes)),
            ExprId::constant(U256::from(no)),
            proof_limits,
        )
        .unwrap()
    };
    let choices_tree = ExprId::ite(
        first_guard.clone(),
        nested(2, 3),
        nested(4, 5),
        proof_limits,
    )
    .unwrap();
    let original_tree = choices_tree.clone();
    assert_eq!(
        specialize_ites(&choices_tree, &BTreeMap::new(), proof_limits).unwrap(),
        choices_tree
    );
    for (first, second, expected) in [
        (true, true, 2),
        (true, false, 3),
        (false, true, 4),
        (false, false, 5),
    ] {
        let choices =
            BTreeMap::from([(first_guard.clone(), first), (second_guard.clone(), second)]);
        assert_eq!(
            specialize_ites(&choices_tree, &choices, proof_limits)
                .unwrap()
                .as_constant(),
            Some(U256::from(expected))
        );
    }
    let true_guard = BTreeMap::from([(first_guard.clone(), true)]);
    let data = ExprId::operation(
        opcode::ADD,
        &[first_guard.clone(), ExprId::constant(U256::from(1))],
        proof_limits,
    )
    .unwrap();
    for term in [&first_guard, &data] {
        assert_eq!(
            specialize_ites(term, &true_guard, proof_limits).unwrap(),
            *term
        );
    }
    let input = ExprId::input(13, Symbol::CallValue);
    let other_input = ExprId::input(14, Symbol::CallValue);
    let source_read = |namespace| {
        ExprId::slot_read(
            SlotStateId::input(SourceId::scoped(input.clone(), U256::from(namespace))),
            ExprId::constant(U256::ZERO),
            proof_limits,
        )
        .unwrap()
    };
    for (left, right) in [
        (input.clone(), other_input),
        (source_read(1), source_read(2)),
    ] {
        let left = AbstractValue::unknown_byte().with_expression(left);
        let right = AbstractValue::unknown_byte().with_expression(right);
        assert_eq!(left.numeric(), right.numeric());
        assert_ne!(
            specialize_ites(left.expression().unwrap(), &true_guard, proof_limits).unwrap(),
            specialize_ites(right.expression().unwrap(), &true_guard, proof_limits).unwrap()
        );
    }
    assert_eq!(
        specialize_ites(
            &choices_tree,
            &true_guard,
            ExprLimits {
                max_nodes: choices_tree.nodes() - 1,
                ..proof_limits
            },
        ),
        Err(ExprError::Limit {
            nodes: choices_tree.nodes(),
            depth: choices_tree.depth(),
        })
    );
    assert_eq!(choices_tree, original_tree);

    let read = |guard| {
        ExprId::ite(
            guard,
            ExprId::constant(U256::ZERO),
            ExprId::constant(U256::from(1)),
            proof_limits,
        )
        .unwrap()
    };
    let add = |amount: u64, count| {
        ExprId::operation(
            opcode::ADD,
            &[ExprId::constant(U256::from(amount)), count],
            proof_limits,
        )
        .unwrap()
    };
    let old_guard = ExprId::ite(
        second_guard.clone(),
        ExprId::constant(U256::from(2)),
        ExprId::constant(U256::MAX),
        proof_limits,
    )
    .unwrap();
    let current_read = read(
        ExprId::ite(
            first_guard.clone(),
            old_guard.clone(),
            source_read(1),
            proof_limits,
        )
        .unwrap(),
    );
    let target_read = read(old_guard.clone());
    let historical_increment = add(1, target_read.clone());
    assert!(increment_cofactor(
        &current_read,
        &historical_increment,
        proof_limits
    ));
    assert_eq!(
        specialize_ites(&current_read, &true_guard, proof_limits).unwrap(),
        target_read
    );
    // A nonzero word guard keeps its actual value; it is never Booleanized.
    for (choice, word) in [(true, U256::from(2)), (false, U256::MAX)] {
        let choices = BTreeMap::from([(first_guard.clone(), true), (second_guard.clone(), choice)]);
        assert_eq!(
            specialize_ites(&old_guard, &choices, proof_limits).unwrap(),
            ExprId::constant(word)
        );
        assert_eq!(
            specialize_ites(&current_read, &choices, proof_limits)
                .unwrap()
                .as_constant(),
            Some(U256::ZERO)
        );
    }
    let reversed = ExprId::ite(
        old_guard.clone(),
        ExprId::constant(U256::from(1)),
        ExprId::constant(U256::ZERO),
        proof_limits,
    )
    .unwrap();
    for invalid in [
        add(2, target_read),
        add(1, reversed),
        add(1, read(add(1, old_guard))),
        add(1, read(source_read(2))),
        add(1, read(ExprId::fresh().unwrap())),
    ] {
        assert!(!increment_cofactor(&current_read, &invalid, proof_limits));
    }
    // The selector occurs inside opaque target data. Choosing it could leave
    // that AST unchanged while still erasing target valuations, so reject it.
    let protected_read = read(
        ExprId::ite(
            first_guard.clone(),
            data.clone(),
            source_read(1),
            proof_limits,
        )
        .unwrap(),
    );
    assert!(!increment_cofactor(
        &protected_read,
        &add(1, read(data)),
        proof_limits
    ));
    assert!(!increment_cofactor(
        &current_read,
        &historical_increment,
        ExprLimits {
            max_nodes: current_read.nodes() - 1,
            ..proof_limits
        }
    ));

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
            assert!(matches!(initial_root.kind(), SlotStateKind::Zero));
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
                let prove_equal = |choices: &BTreeMap<ExprId, bool>,
                                   left: &ExprId,
                                   right: &ExprId,
                                   leaf: &SlotStateId,
                                   obligation: &str| {
                    let specialize = |term: &ExprId| {
                        specialize_ites(term, choices, limits.expressions()).unwrap_or_else(|error| {
                            panic!(
                                "{fork:?} state{} slot {slot} root{} leaf{} {obligation}: ITE specialization: {error:?}; guards={} nodes={} depth={} limits={:?}",
                                outcome.state, persistent_root.id(), leaf.id(), choices.len(), term.nodes(), term.depth(), limits.expressions()
                            )
                        })
                    };
                    assert_eq!(
                        specialize(left),
                        specialize(right),
                        "{fork:?} state{} slot {slot} root{} leaf{} {obligation}: structural mismatch; guards={} left nodes={} depth={} right nodes={} depth={}",
                        outcome.state,
                        persistent_root.id(),
                        leaf.id(),
                        choices.len(),
                        left.nodes(),
                        left.depth(),
                        right.nodes(),
                        right.depth()
                    );
                };
                // A1's common must-table is not a historical value archive.
                // Independently pair each leaf with the actual complete root
                // by ITE denotation under its entire exact selector path. No
                // payload facts or desired equality are assumed by this proof.
                let mut pending = vec![(
                    &persistent_root,
                    3,
                    BTreeMap::<ExprId, bool>::new(),
                    None::<(ExprId, bool, bool)>,
                    None::<(&SlotStateId, ExprId)>,
                )];
                let mut visited = 0;
                let mut leaves = 0;
                while let Some((state, upper, choices, conflict, selected)) = pending.pop() {
                    // Count expanded visits: equal nodes reached by different
                    // selector paths must never discard each other's guards.
                    visited += 1;
                    assert!(
                        visited <= persistent_root.nodes(),
                        "{fork:?} state{} slot {slot} root{}: persistent path visits={visited} nodes={} depth={}",
                        outcome.state,
                        persistent_root.id(),
                        persistent_root.nodes(),
                        persistent_root.depth()
                    );
                    match state.kind() {
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
                            let selected = if written == slot {
                                assert!(selected.is_none(), "duplicate persistent slot write");
                                Some((state, value.clone()))
                            } else {
                                selected
                            };
                            // Validate the remaining order and initial root
                            // even after finding this slot's authoritative write.
                            pending.push((previous, written, choices, conflict, selected));
                            continue;
                        }
                        SlotStateKind::Ite {
                            condition,
                            then_state,
                            else_state,
                        } => {
                            for (next, nonzero) in [(then_state, true), (else_state, false)] {
                                let mut branch = choices.clone();
                                let mut conflict = conflict.clone();
                                if let Some(previous) = branch.get(condition) {
                                    if *previous != nonzero && conflict.is_none() {
                                        conflict = Some((condition.clone(), *previous, nonzero));
                                    }
                                } else {
                                    branch.insert(condition.clone(), nonzero);
                                }
                                // Keep the first opposite choices as an exact
                                // no-model witness; never overwrite a guard.
                                pending.push((next, upper, branch, conflict, selected.clone()));
                            }
                            continue;
                        }
                        SlotStateKind::Zero => {
                            assert_eq!(state, &initial_root);
                        }
                        SlotStateKind::Input { .. } => panic!("lost initial persistent root"),
                    }
                    if let Some((condition, previous, nonzero)) = conflict {
                        assert_ne!(previous, nonzero);
                        assert_eq!(choices.get(&condition), Some(&previous));
                        eprintln!(
                            "{fork:?} state{} slot {slot} root{}: exact contradictory path guard={condition:?}, choices={previous}/{nonzero}",
                            outcome.state,
                            persistent_root.id()
                        );
                        continue;
                    }
                    let (leaf, value) = selected.unwrap_or((state, ExprId::constant(U256::ZERO)));
                    leaves += 1;
                    prove_equal(&choices, &expression, &value, leaf, "root/leaf value");
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
                                    &choices,
                                    term,
                                    retained,
                                    leaf,
                                    "optional operand expression",
                                );
                            }
                            if let Some(constant) = term.as_constant() {
                                assert!(guarantee.contains(constant));
                            } else {
                                // The structural witness equates this term and
                                // the read on this path. Intersect their numeric
                                // product bounds over the complete finite read;
                                // a surviving word is not a path model.
                                assert!(
                                    read.constants().unwrap().iter().any(|candidate| {
                                        read.contains(*candidate) && guarantee.contains(*candidate)
                                    }),
                                    "{fork:?} state{} slot {slot} root{} leaf{}: disjoint optional/read numeric products: {:?} / {:?}",
                                    outcome.state,
                                    persistent_root.id(),
                                    leaf.id(),
                                    guarantee.numeric(),
                                    read.numeric()
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
            // Each retained increment must read an actual owner201 input root
            // or an exact target-disjoint ITE cofactor of its current read.
            let current_read = old_count.symbolic_expression().unwrap();
            for historical in actual_writes.difference(&increments) {
                assert!(
                    increment_cofactor(&current_read, historical, limits.expressions()),
                    "{fork:?}: missing exact historical increment cofactor: {historical:?}"
                );
            }
            assert_eq!(
                writes(&histories[1]),
                std::collections::BTreeSet::from([ExprId::constant(U256::from(1))])
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
            // The fixture's full F must be True. A shared exact cover replaces
            // the large local can_equal acceptance queries: it proves common
            // positive assignments and every negative control on this SAME
            // nine-tuple, including returned values without route qualification
            // and a rejected second CALL retaining the first CALL's memory.
            let terms = std::array::from_fn(|index| match index {
                0..=2 => persistent[index].symbolic_expression().unwrap(),
                3..=6 => counters[index - 3].symbolic_expression().unwrap(),
                _ => routes[index - 7].clone(),
            });
            let cover = joint::verify(&payload.relations, &terms, limits.expressions())
                .unwrap_or_else(|error| {
                    panic!(
                        "{fork:?} state{} shared joint cover: {error}",
                        outcome.state
                    )
                });
            eprintln!(
                "{fork:?} state{} shared joint cover: {} proof nodes, {} terminal vectors",
                outcome.state, cover.proof_nodes, cover.terminal_vectors
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
