//! 在真实世界执行中核对冻结策略、资源停止、摘要 guard 与循环闭包。

use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        Config, ExecutionConfig, FrontierReason, Status, SummaryInput, WorldAnalysis, analyze_world,
    },
    domain::{
        AbstractValue, Domain, DomainSpec, Profile, ReductionStatus,
        facts::{BitIndex, UnaryPredicate, WordBounds},
    },
    ssa,
    world::{Account, ByteArray, Entry, World},
};
use revm_bytecode::opcode;
use std::num::NonZeroUsize;

#[path = "domain_policy/rounds.rs"]
mod rounds;

fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

fn domain(profile: Profile, capacity: usize, rounds: usize, facts: usize) -> Domain {
    Domain::from_spec(DomainSpec::new(
        profile,
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(rounds).unwrap(),
        NonZeroUsize::new(facts).unwrap(),
    ))
}

fn fixture(code: &str) -> (World, Entry) {
    let mut world = World::offline(Fork::Osaka, "domain-policy:v1", "domain policy test");
    let mut account = Account::from_hex(code, world.fork()).unwrap();
    account.balance = word(1_000_000);
    world.insert(address(0x101), account).unwrap();
    let entry = Entry {
        address: address(0x101),
        environment: evm_abstract::world::EvmEnvironment {
            to: (address(0x101)).into(),
            caller: (address(0x900)).into(),
            value: word(0),
            calldata: ByteArray::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironment::default()
        },
    };
    (world, entry)
}

fn assert_complete(graph: &WorldAnalysis) {
    assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
    ssa::build_world(graph).unwrap().verify(graph).unwrap();
}

#[test]
fn fingerprints_bind_original_open_constraints_across_runtime_profiles() {
    let product = domain(Profile::Product, 1, 4, 256);
    let bounds =
        UnaryPredicate::UnsignedBounds(WordBounds::new(U256::ZERO, U256::from(255)).unwrap());
    let odd = product
        .from_facts(&[
            bounds.clone(),
            UnaryPredicate::BitSet(BitIndex::new(0).unwrap()),
        ])
        .unwrap();
    let even = product
        .from_facts(&[bounds, UnaryPredicate::BitClear(BitIndex::new(0).unwrap())])
        .unwrap();
    assert!(odd.constants().is_none() && even.constants().is_none());
    let (mut world, entry) = fixture("00");
    let mut account = world.accounts()[&entry.address].clone();
    account.storage.insert(U256::ZERO, odd.clone());
    let mut odd_world = World::offline(Fork::Osaka, "domain-policy:v1", "domain policy test");
    odd_world.insert(entry.address, account.clone()).unwrap();
    account.storage.insert(U256::ZERO, even);
    let mut even_world = World::offline(Fork::Osaka, "domain-policy:v1", "domain policy test");
    even_world.insert(entry.address, account.clone()).unwrap();
    account.storage.insert(U256::ZERO, AbstractValue::top());
    // 每个世界都绑定同一账户、代码和快照；只有开放数值约束不同。
    world = World::offline(Fork::Osaka, "domain-policy:v1", "domain policy test");
    world.insert(entry.address, account).unwrap();
    assert_ne!(
        odd_world.fingerprint().unwrap(),
        even_world.fingerprint().unwrap()
    );
    assert_ne!(
        odd_world.fingerprint().unwrap(),
        world.fingerprint().unwrap()
    );
    assert_ne!(
        even_world.fingerprint().unwrap(),
        world.fingerprint().unwrap()
    );
    let original_fingerprint = odd_world.fingerprint().unwrap();
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let graph = analyze_world(
            odd_world.clone(),
            entry.clone(),
            ExecutionConfig {
                analysis: Config {
                    domain_profile: profile,
                    max_constants: 1,
                    ..Config::default()
                },
                ..ExecutionConfig::default()
            },
        )
        .unwrap();
        assert_complete(&graph);
        assert_eq!(graph.world().fingerprint().unwrap(), original_fingerprint);
        assert_eq!(
            graph.world().accounts()[&entry.address].storage[&U256::ZERO],
            odd
        );
        let stored = graph.states()[0]
            .entry
            .store
            .read(entry.address, &word(0), product);
        assert!(stored.contains(U256::from(1)));
        assert_eq!(
            stored.contains(U256::ZERO),
            profile == Profile::ConstantsOnly
        );
    }
}

#[test]
fn json_distinguishes_a_top_component_from_the_whole_product() {
    let product = domain(Profile::Product, 1, 4, 256);
    let byte = product
        .from_facts(&[UnaryPredicate::UnsignedBounds(
            WordBounds::new(U256::ZERO, U256::from(255)).unwrap(),
        )])
        .unwrap();
    assert!(byte.constants().is_none());
    assert!(byte.congruence().is_top());
    let byte_json = serde_json::to_value(&byte).unwrap();
    assert!(byte_json.is_object());
    assert_eq!(byte_json["congruence"], "Top");
    assert!(byte_json.get("Constants").is_none());
    assert!(byte_json["known_bits"].is_object());
    assert!(byte_json["interval"].is_object());
    assert_eq!(serde_json::to_value(AbstractValue::top()).unwrap(), "Top");
    let (world, entry) = fixture("00");
    let graph = analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    let json = serde_json::to_value(&graph).unwrap();
    assert_eq!(
        json["schema_version"],
        evm_abstract::analysis::SCHEMA_VERSION
    );
    assert_eq!(
        json["domain_spec"],
        serde_json::to_value(graph.domain_spec()).unwrap()
    );
    assert_eq!(json["domain_spec"]["profile"], "product");
    assert_eq!(json["domain_spec"]["word_bits"], 256);
    // Billing policy 6 does not change the domain policy's serialized schema.
    assert_eq!(json["domain_spec"]["schema_version"], 2);
    assert_eq!(json["domain_spec"]["cost_version"], 6);
    assert_eq!(json["domain_spec"]["widening_after_updates"], 2);
}

#[test]
fn insufficient_initial_work_retains_a_typed_source_free_frontier() {
    let (world, entry) = fixture("00");
    let graph = analyze_world(
        world,
        entry,
        ExecutionConfig {
            max_work: 1,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(graph.status(), Status::Incomplete);
    assert!(graph.states().is_empty());
    assert!(graph.outcomes().is_empty());
    assert_eq!(graph.frontiers().len(), 1);
    let frontier = &graph.frontiers()[0];
    assert_eq!(frontier.from, None);
    assert_eq!(frontier.target, None);
    assert_eq!(frontier.pc, None);
    assert_eq!(frontier.reason, FrontierReason::Work);
    assert!(graph.work() <= 1);
    assert!(ssa::build_world(&graph).is_err());
}

#[test]
fn successive_fact_rounds_materialize_a_real_cross_domain_chain() {
    let one_round = domain(Profile::Product, 1, 1, 256);
    // Bounds consumes modulo 3 first: 0..18, then low bits 01 give 1..17.
    // Scalar exports those low bits as modulo 4; CRT produces 9 modulo 12.
    // Only a later bounds evaluation can align that NEW congruence to 9..9.
    let partially_reduced = one_round
        .from_facts(&[
            UnaryPredicate::UnsignedBounds(WordBounds::new(U256::ZERO, U256::from(20)).unwrap()),
            UnaryPredicate::congruent(U256::from(3), U256::ZERO).unwrap(),
            UnaryPredicate::BitSet(BitIndex::new(0).unwrap()),
            UnaryPredicate::BitClear(BitIndex::new(1).unwrap()),
        ])
        .unwrap();
    assert_eq!(
        partially_reduced.interval().unsigned_bounds(),
        (U256::ONE, U256::from(17))
    );
    assert_eq!(
        partially_reduced.congruence().modulus_residue(),
        Some((U256::from(12), U256::from(9)))
    );
    assert!(partially_reduced.constants().is_none());
    assert_eq!(partially_reduced.singleton(), None);
    // This integer enumeration is independent of every abstract component.
    let witnesses: Vec<_> = (0_u64..=20).filter(|x| x % 3 == 0 && x & 3 == 1).collect();
    assert_eq!(witnesses, [9]);
    let next = one_round.reduce(&partially_reduced);
    assert_eq!(next.status, ReductionStatus::RoundLimit);
    assert_eq!(next.rounds, 1);
    assert!(next.strengthened > 0);
    let exact = Some(U256::from(9));
    assert_eq!(next.value.singleton(), exact);
    assert_eq!(next.value.finite_constants().singleton(), exact);
    assert_eq!(next.value.known_bits().singleton(), exact);
    assert_eq!(next.value.interval().singleton(), exact);
    assert_eq!(next.value.congruence().singleton(), exact);
    let full = domain(Profile::Product, 1, 8, 256).reduce(&partially_reduced);
    assert_eq!(full.status, ReductionStatus::Stable);
    assert!(full.rounds >= 2);
    assert_eq!(full.value, next.value);
    assert_eq!(
        one_round.reduce(&full.value).status,
        ReductionStatus::Stable
    );
    // All possible members lie in the asserted bounds above, so this also
    // checks preservation of the whole joint concrete set through feedback.
    for value in [&partially_reduced, &next.value, &full.value] {
        for raw in 0_u64..=31 {
            assert_eq!(value.contains(U256::from(raw)), witnesses.contains(&raw));
        }
    }
    // 从不把规约写入 join：有限容量已溢出，两个原始分量的界仍是 21..33。
    let raw_join = one_round.join(&word(21), &word(33));
    assert_eq!(
        raw_join.interval().unsigned_bounds(),
        (U256::from(21), U256::from(33))
    );
}

#[test]
fn fact_capacity_and_unquotable_work_stop_feedback_at_distinct_boundaries() {
    let value =
        Domain::default().apply(opcode::BYTE, &[AbstractValue::top(), AbstractValue::top()]);
    for (rounds, status) in [
        (1, ReductionStatus::FactLimit),
        (8, ReductionStatus::FactLimit),
        (64_000, ReductionStatus::FactLimit),
        // A step-count overflow cannot be quoted or paid. Refuse it before
        // executing rules; it is distinct from the later fact-capacity stop.
        (usize::MAX, ReductionStatus::WorkLimit),
    ] {
        let tiny = domain(Profile::Product, 1, rounds, 1);
        let stopped = tiny.reduce(&value);
        assert_eq!(stopped.status, status, "rounds={rounds}");
        assert_eq!(stopped.rounds, 0, "rounds={rounds}");
        assert_eq!(stopped.value, value, "rounds={rounds}");
        assert!(stopped.value.contains(U256::ZERO), "rounds={rounds}");
        assert!(stopped.value.contains(U256::from(255)), "rounds={rounds}");
        assert!(!stopped.value.contains(U256::from(256)), "rounds={rounds}");
    }
}

fn read_only_summary() -> WorldAnalysis {
    let caller = "5f5f5f5f5f6102006207a120f1505f5f5f5f5f6102006207a120f15000";
    let (mut world, entry) = fixture(caller);
    world
        .insert(
            address(0x200),
            Account::from_hex("00", world.fork()).unwrap(),
        )
        .unwrap();
    let graph = analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    assert_complete(&graph);
    assert!(
        graph.summary_stats().hits > 0,
        "{:?}",
        graph.summary_stats()
    );
    graph
}

#[test]
fn summaries_guard_the_entire_frozen_domain_policy() {
    let graph = read_only_summary();
    let spec = graph.domain_spec();
    let input: &SummaryInput = &graph.summaries()[0].input;
    for record in graph.summaries() {
        assert_eq!(record.input.domain_spec, spec);
        assert_eq!(
            record.input.world_fingerprint,
            graph.world().fingerprint().unwrap()
        );
        let json = serde_json::to_value(&record.input).unwrap();
        assert_eq!(json["domain_spec"]["schema_version"], 2);
        assert_eq!(json["domain_spec"]["cost_version"], 6);
        assert_eq!(json["domain_spec"]["widening_after_updates"], 2);
        assert_eq!(
            json["domain_spec"]["provenance_policy"],
            "scoped-expressions-and-value-identities-v3"
        );
    }
    for changed in [
        domain(
            Profile::ConstantsOnly,
            spec.capacity(),
            spec.reduction_rounds(),
            spec.fact_limit(),
        )
        .spec(),
        domain(
            Profile::Product,
            spec.capacity() + 1,
            spec.reduction_rounds(),
            spec.fact_limit(),
        )
        .spec(),
        domain(
            Profile::Product,
            spec.capacity(),
            spec.reduction_rounds() + 1,
            spec.fact_limit(),
        )
        .spec(),
        domain(
            Profile::Product,
            spec.capacity(),
            spec.reduction_rounds(),
            spec.fact_limit() + 1,
        )
        .spec(),
    ] {
        let mut different = input.clone();
        different.domain_spec = changed;
        assert_ne!(
            &different, input,
            "changing policy must make an exact guard miss"
        );
    }
}

#[test]
fn a_counter_loop_reaches_a_stored_postfixpoint_and_verifies_ssa() {
    use evm_abstract::analysis::{EdgeKind, MachineEdgeKind};

    let (world, entry) = fixture("5f5b600101600156");
    let graph = analyze_world(
        world,
        entry,
        ExecutionConfig {
            analysis: Config {
                context_depth: 0,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_complete(&graph);
    assert!(graph.frontiers().is_empty());
    let loop_state = graph
        .states()
        .iter()
        .find(|state| {
            state.program().unwrap().blocks()[state.active().basic_block_index].start_pc == 1
        })
        .unwrap();
    let block = &loop_state.program().unwrap().blocks()[loop_state.active().basic_block_index];
    assert_eq!(
        block
            .instructions
            .iter()
            .map(|instruction| instruction.opcode)
            .collect::<Vec<_>>(),
        [
            opcode::JUMPDEST,
            opcode::PUSH1,
            opcode::ADD,
            opcode::PUSH1,
            opcode::JUMP
        ]
    );
    assert_eq!(block.instructions[1].immediate, Some(U256::ONE));
    assert_eq!(block.instructions[3].immediate, Some(U256::ONE));
    assert_eq!(
        loop_state.program().unwrap().jumpdest_blocks().get(&1),
        Some(&loop_state.active().basic_block_index)
    );
    assert_eq!(
        loop_state.executed_pcs,
        block
            .instructions
            .iter()
            .map(|instruction| instruction.pc)
            .collect::<Vec<_>>()
    );
    assert!(loop_state.active().jump_history.is_empty());
    let backedge_kind = MachineEdgeKind::Intraprocedural(EdgeKind::Jump);
    assert!(graph.edges().iter().any(|edge| edge.from == loop_state.id
        && edge.to == loop_state.id
        && edge.kind == backedge_kind));
    let evidence = loop_state.execution_evidence().unwrap();
    assert!(evidence.is_current());
    assert!(
        evidence
            .successors()
            .contains(&(loop_state.id, backedge_kind))
    );

    assert_eq!(loop_state.entry.active().stack.len(), 1);
    let input = &loop_state.entry.active().stack[0];
    let exit = loop_state.exit.as_ref().unwrap();
    assert_eq!(exit.key(), loop_state.key);
    assert_eq!(exit.store, loop_state.entry.store);
    assert_eq!(exit.active().stack, loop_state.exit_stack);
    assert_eq!(exit.active().stack.len(), 1);
    let transferred = &exit.active().stack[0];
    let add_result = evidence
        .instruction_values()
        .unwrap()
        .iter()
        .find(|value| {
            loop_state.executed_pcs[value.instruction_index()] == block.instructions[2].pc
        })
        .unwrap();
    assert_eq!(add_result.numeric(), transferred.numeric());

    let domain = Domain::from_spec(graph.domain_spec());
    let next = domain.apply(opcode::ADD, &[word(1), input.clone()]);
    assert_eq!(transferred.numeric(), next.numeric());
    // Loop closure orders numeric bounds; independent runtime expression roots
    // need not survive a join with the expression produced by ADD.
    let alpha = |value: &AbstractValue| AbstractValue::from_numeric(value.numeric().clone());
    let bound = alpha(input);
    for successor in [&next, transferred] {
        assert_eq!(
            domain.join(&bound, &alpha(successor)).numeric(),
            bound.numeric(),
            "stored numeric entry must cover its actual backedge transfer"
        );
    }
    assert!(input.contains(U256::ZERO));
    assert!(input.contains(U256::MAX));
    for counter in [
        U256::ZERO,
        U256::ONE,
        U256::from(255),
        U256::MAX - U256::ONE,
        U256::MAX,
    ] {
        let successor = counter.wrapping_add(U256::ONE);
        assert!(input.contains(counter));
        assert!(transferred.contains(successor));
        assert!(bound.contains(successor));
        assert_eq!(
            domain
                .apply(opcode::ADD, &[word(1), AbstractValue::constant(counter)])
                .singleton(),
            Some(successor)
        );
    }
    assert_eq!(
        domain.join(&word(1), &word(2)).interval().unsigned_bounds(),
        (U256::from(1), U256::from(2))
    );
}

#[test]
fn summary_replay_does_not_turn_separate_return_reads_into_copy_identity() {
    use evm_abstract::{
        analysis::OutcomeKind,
        domain::{relational::CheckResult, symbolic::ExprId},
    };

    let call = |offset: u8| format!("602060{offset:02x}5f5f5f6102006207a120f1");
    let counter_caller = format!("{}50{}50{}506020516040511800", call(0), call(32), call(64));
    // A successful counter call changes the ordered symbolic store even though
    // its numeric projection remains Top. Those exact preconditions must miss.
    // A rejected earlier CALL leaves the original input and can validly reuse it.
    // GAS produces a new unknown output without changing any input. A warm-up
    // with no output copy lets both compared calls replay the same certificate.
    let readonly_caller = format!(
        "5f5f5f5f5f6102006207a120f150{}50{}505f516020511800",
        call(0),
        call(32)
    );
    for (caller, callee_code, reusable) in [
        (counter_caller, "5f54806001015f555f5260205ff3", false),
        (readonly_caller, "5a5f5260205ff3", true),
    ] {
        let (mut world, entry) = fixture(&caller);
        let mut callee = Account::from_hex(callee_code, world.fork()).unwrap();
        callee.storage_unknown = true;
        world.insert(address(0x200), callee).unwrap();
        let mut previous = None;
        for use_summaries in [false, true] {
            let graph = analyze_world(
                world.clone(),
                entry.clone(),
                ExecutionConfig {
                    use_summaries,
                    ..ExecutionConfig::default()
                },
            )
            .unwrap();
            assert_complete(&graph);
            if use_summaries && reusable {
                assert!(
                    graph.summary_stats().hits >= 2,
                    "{:?}",
                    graph.summary_stats()
                );
                assert!(
                    graph
                        .summaries()
                        .iter()
                        .any(|record| record.reused_at.len() >= 2)
                );
            } else if !use_summaries {
                assert_eq!(graph.summary_stats().hits, 0);
            }
            if use_summaries && !reusable {
                let initial = &graph.states()[0].entry.store;
                let reused: Vec<_> = graph
                    .summaries()
                    .iter()
                    .filter(|record| !record.reused_at.is_empty())
                    .collect();
                assert_eq!(reused.len(), 1);
                // CALL snapshots rollback state before touching the callee.
                let mut entered = initial.clone();
                entered.touch_account(address(0x200));
                assert_eq!(reused[0].input.store, entered);
                assert_eq!(reused[0].input.frame.state.saved_store.state(), initial);
                assert_eq!(
                    graph.states()[reused[0].source_state].key.frames[0].basic_block_index,
                    0
                );
                for site in &reused[0].reused_at {
                    assert_eq!(graph.states()[*site].key.frames[0].basic_block_index, 1);
                    assert_eq!(
                        graph.states()[*site].active().code_hash,
                        reused[0].input.frame.state.key.code_hash
                    );
                }
                let admitted_sites: std::collections::BTreeSet<_> = graph
                    .summaries()
                    .iter()
                    .map(|record| {
                        graph.states()[record.source_state].key.frames[0].basic_block_index
                    })
                    .collect();
                assert_eq!(admitted_sites, std::collections::BTreeSet::from([0, 1, 2]));
                for record in graph.summaries() {
                    assert!(!record.outputs.is_empty());
                    let mut has_return = false;
                    let mut has_failure = false;
                    for output in &record.outputs {
                        match output.kind {
                            OutcomeKind::Return => {
                                has_return = true;
                                assert_ne!(
                                    output.store.slot_root(address(0x200), false).unwrap(),
                                    record.input.store.slot_root(address(0x200), false).unwrap()
                                );
                            }
                            OutcomeKind::Failure => {
                                has_failure = true;
                                // Memory gas failures restore the full pre-entry Store.
                                assert_eq!(
                                    &output.store,
                                    record.input.frame.state.saved_store.state()
                                );
                                assert_eq!(output.data, ByteArray::empty());
                            }
                            OutcomeKind::Revert => panic!("counter callee cannot revert"),
                        }
                    }
                    assert!(has_return, "counter summary must retain a Return");
                    assert!(has_failure, "counter summary must retain a Failure");
                }
                for pair in graph.summaries().windows(2) {
                    assert_ne!(pair[0].input.store, pair[1].input.store);
                    assert_ne!(
                        pair[0].input.frame.state.saved_store,
                        pair[1].input.frame.state.saved_store
                    );
                    assert_ne!(
                        pair[0]
                            .input
                            .store
                            .slot_root(address(0x200), false)
                            .unwrap(),
                        pair[1]
                            .input
                            .store
                            .slot_root(address(0x200), false)
                            .unwrap()
                    );
                }
            }
            let final_state = graph
                .states()
                .iter()
                .find(|state| {
                    state.active().code_address == entry.address
                        && state
                            .entry
                            .active()
                            .stack
                            .last()
                            .is_some_and(|value| value.singleton() == Some(U256::from(1)))
                        && state
                            .executed_pcs
                            .contains(&(state.program().unwrap().byte_len() - 1))
                })
                .unwrap();
            let result = &final_state.exit_stack[0];
            assert_eq!(result.singleton(), None);
            assert!(result.contains(U256::ZERO));
            assert!(result.contains(U256::from(1)));
            assert!(result.contains(U256::MAX));
            let observable = (result.numeric().clone(), result.provenance().clone());
            if let Some(before) = &previous {
                assert_eq!(&observable, before);
            }
            previous = Some(observable);

            if reusable {
                let domain = Domain::from_spec(graph.domain_spec());
                let limits = graph.domain_spec().relations();
                let terminal = final_state.exit.as_ref().unwrap();
                let words = [0, 32].map(|offset| {
                    terminal
                        .active()
                        .memory
                        .read_word_checked(&word(offset), domain)
                        .unwrap()
                });
                assert!(!words[0].identity().same_identity(words[1].identity()));
                let equal = |value: &AbstractValue, constant: u64| {
                    ExprId::operation(
                        opcode::EQ,
                        &[
                            value.symbolic_expression().unwrap(),
                            ExprId::constant(U256::from(constant)),
                        ],
                        limits.expressions(),
                    )
                    .unwrap()
                };
                let and = |left, right| {
                    ExprId::operation(opcode::AND, &[left, right], limits.expressions()).unwrap()
                };
                let continuations: Vec<_> = graph
                    .states()
                    .iter()
                    .filter(|state| {
                        state.key.frames.len() == 1
                            && state.active().basic_block_index >= 2
                            && state.entry.active().stack.last().unwrap().singleton()
                                == Some(U256::from(1))
                    })
                    .collect();
                assert_eq!(continuations.len(), 2);
                let call_success = continuations.iter().fold(
                    ExprId::constant(U256::from(1)),
                    |combined, state| {
                        and(
                            combined,
                            equal(state.entry.active().stack.last().unwrap(), 1),
                        )
                    },
                );
                assert_eq!(
                    terminal
                        .relations
                        .can_equal(&call_success, U256::from(1), &limits),
                    CheckResult::MayBeFeasible
                );
                for right in [1, 2] {
                    let pair = and(
                        call_success.clone(),
                        and(equal(&words[0], 1), equal(&words[1], right)),
                    );
                    assert_eq!(
                        terminal.relations.can_equal(&pair, U256::from(1), &limits),
                        CheckResult::MayBeFeasible
                    );
                }
            }
        }
    }
}

#[test]
fn low160_projection_resolves_a_call_even_when_the_raw_word_is_open() {
    let mask = U256::MAX >> 96usize;
    let target = Domain::default()
        .from_facts(&[UnaryPredicate::KnownBits(
            evm_abstract::domain::facts::BitConstraints::new(
                mask & !U256::from(0x200),
                U256::from(0x200),
            )
            .unwrap(),
        )])
        .unwrap();
    assert!(target.constants().is_none() && target.singleton().is_none());
    assert_eq!(
        Domain::default().address_projection(&target).singleton(),
        Some(U256::from(0x200))
    );
    let (mut world, entry) = fixture("5f5f5f5f5f5f5461fffff100");
    let mut root = world.account(address(0x101)).unwrap().clone();
    root.storage.insert(U256::ZERO, target);
    // Replace through a fresh immutable world, since conflicting observations are rejected.
    world = World::new(Fork::Osaka, "low160 projection");
    world.insert(address(0x101), root).unwrap();
    world
        .insert(
            address(0x200),
            Account::from_hex("00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let graph = analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
    assert!(
        graph
            .edges()
            .iter()
            .any(|edge| edge.kind == evm_abstract::analysis::MachineEdgeKind::Call)
    );
    ssa::build_world(&graph).unwrap().verify(&graph).unwrap();
}

#[test]
fn projecting_a_larger_finite_input_obeys_the_frozen_constants_capacity() {
    let wide = domain(Profile::Product, 64, 4, 256);
    let input = (1_u64..=9).fold(word(0), |a, v| wide.join(&a, &word(v)));
    assert_eq!(input.constants().unwrap().len(), 10);
    let constants = domain(Profile::ConstantsOnly, 8, 4, 256).project(&input);
    assert_eq!(
        constants.numeric(),
        &evm_abstract::domain::NumericValue::top()
    );
    assert_eq!(constants.provenance(), input.provenance());
    let product = domain(Profile::Product, 8, 4, 256).project(&input);
    assert!(product.constants().is_none());
    for v in 0_u64..=9 {
        assert!(product.contains(U256::from(v)));
    }
}

#[test]
fn a_budgeted_domain_operation_uses_the_supplied_root_ledger_transactionally() {
    let domain = Domain::default();
    let mut budget = evm_abstract::resource::WorkBudget::new(0);
    assert_eq!(
        domain
            .apply_budgeted(opcode::ADD, &[word(1), word(2)], &mut budget)
            .unwrap_err(),
        evm_abstract::domain::WorkExhausted
    );
    assert!(budget.exhausted());
    assert_eq!(budget.used(), 0);
    let mut budget = evm_abstract::resource::WorkBudget::new(10_000);
    let result = domain
        .apply_budgeted(opcode::ADD, &[word(1), word(2)], &mut budget)
        .unwrap();
    assert_eq!(result.value.singleton(), Some(U256::from(3)));
    assert!(budget.used() > 0);
}

#[test]
fn block_local_copy_identities_do_not_escape_into_saved_graph_values() {
    let (world, mut entry) = fixture("5f356001018000");
    // Arithmetic produces a temporary definition; root calldata itself now has
    // a stable environment symbol that intentionally survives block boundaries.
    entry.environment.calldata = ByteArray::unknown();
    let graph = analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    let stack = &graph.states()[0].exit_stack;
    assert_eq!(stack.len(), 2);
    assert!(!stack[0].identity().same_identity(stack[1].identity()));
    // The runtime definition IDs are gone, while the independent persistent
    // expression can now prove the derived-value equality.
    let symbolic = Domain::default().apply(opcode::XOR, &stack.clone());
    assert_eq!(symbolic.singleton(), Some(U256::ZERO));
    let numeric = Domain::from_spec(Domain::default().spec().with_relations(
        evm_abstract::domain::relational::RelationLimits {
            enabled: false,
            ..Default::default()
        },
    ))
    .apply(opcode::XOR, &stack.clone());
    // Without the independent expression layer these saved slots supply no
    // reusable runtime-copy proof; both scalar outcomes remain possible.
    assert!(numeric.contains(U256::ZERO) && numeric.contains(U256::from(1)));
}

#[test]
fn absent_account_validation_uses_numeric_zero_independently_of_provenance() {
    let mut world = World::new(Fork::Osaka, "zero provenance");
    let mut absent = Account::absent();
    absent.balance = word(0).with_origin(evm_abstract::domain::provenance::Origin::Balance);
    absent.nonce = word(0).with_origin(evm_abstract::domain::provenance::Origin::Nonce);
    world.insert(address(0x200), absent).unwrap();
}
