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
    assert_ne!(odd_world.fingerprint(), even_world.fingerprint());
    assert_ne!(odd_world.fingerprint(), world.fingerprint());
    assert_ne!(even_world.fingerprint(), world.fingerprint());
    let original_fingerprint = odd_world.fingerprint();
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
        assert_eq!(graph.world().fingerprint(), original_fingerprint);
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
    assert_eq!(json["schema_version"], 3);
    assert_eq!(
        json["domain_spec"],
        serde_json::to_value(graph.domain_spec()).unwrap()
    );
    assert_eq!(json["domain_spec"]["profile"], "product");
    assert_eq!(json["domain_spec"]["word_bits"], 256);
    assert_eq!(json["domain_spec"]["cost_version"], 2);
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
    // 同余给 21..33；bit 4 clear 把范围收紧为 32..33。第二轮再次对齐同余，
    // 才能把唯一成员 33 导入常量、位、区间和同余四个组件。
    let partially_reduced = one_round
        .from_facts(&[
            UnaryPredicate::UnsignedBounds(
                WordBounds::new(U256::from(10), U256::from(40)).unwrap(),
            ),
            UnaryPredicate::congruent(U256::from(12), U256::from(9)).unwrap(),
            UnaryPredicate::BitClear(BitIndex::new(4).unwrap()),
        ])
        .unwrap();
    assert_eq!(
        partially_reduced.interval().unsigned_bounds(),
        (U256::from(32), U256::from(33))
    );
    assert!(partially_reduced.constants().is_none());
    assert_eq!(partially_reduced.singleton(), None);
    assert!(partially_reduced.contains(U256::from(33)));
    let next = one_round.reduce(&partially_reduced);
    assert_eq!(next.status, ReductionStatus::RoundLimit);
    assert_eq!(next.rounds, 1);
    assert!(next.strengthened > 0);
    assert_eq!(next.value.singleton(), Some(U256::from(33)));
    let full = domain(Profile::Product, 1, 8, 256).reduce(&partially_reduced);
    assert_eq!(full.status, ReductionStatus::Stable);
    assert!(full.rounds >= 2);
    assert_eq!(full.value, next.value);
    assert_eq!(
        one_round.reduce(&full.value).status,
        ReductionStatus::Stable
    );
    // 从不把规约写入 join：有限容量已溢出，两个原始分量的界仍是 21..33。
    let raw_join = one_round.join(&word(21), &word(33));
    assert_eq!(
        raw_join.interval().unsigned_bounds(),
        (U256::from(21), U256::from(33))
    );
}

#[test]
fn fact_capacity_stops_feedback_even_with_an_unbounded_round_setting() {
    let value =
        Domain::default().apply(opcode::BYTE, &[AbstractValue::top(), AbstractValue::top()]);
    let tiny = domain(Profile::Product, 1, usize::MAX, 1);
    for _ in 0..16 {
        let stopped = tiny.reduce(&value);
        assert_eq!(stopped.status, ReductionStatus::FactLimit);
        assert_eq!(stopped.rounds, 0);
        assert_eq!(stopped.value, value);
        assert!(stopped.value.contains(U256::ZERO));
        assert!(stopped.value.contains(U256::from(255)));
        assert!(!stopped.value.contains(U256::from(256)));
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
        assert_eq!(record.input.world_fingerprint, graph.world().fingerprint());
        let json = serde_json::to_value(&record.input).unwrap();
        assert_eq!(json["domain_spec"]["schema_version"], 2);
        assert_eq!(json["domain_spec"]["cost_version"], 2);
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
    let loop_state = graph
        .states()
        .iter()
        .find(|state| {
            state.program().unwrap().blocks()[state.active().basic_block_index].start_pc == 1
        })
        .unwrap();
    let input = &loop_state.entry.active().stack[0];
    let domain = Domain::from_spec(graph.domain_spec());
    let next = domain.apply(opcode::ADD, &[word(1), input.clone()]);
    assert_eq!(
        domain.join(input, &next),
        *input,
        "stored entry must cover its own backedge transfer"
    );
    assert!(input.contains(U256::ZERO));
    assert!(input.contains(U256::MAX));
    assert_eq!(
        domain.join(&word(1), &word(2)).interval().unsigned_bounds(),
        (U256::from(1), U256::from(2))
    );
}

#[test]
fn summary_replay_does_not_turn_separate_return_reads_into_copy_identity() {
    let call = |offset: u8| format!("602060{offset:02x}5f5f5f6102006207a120f1");
    let caller = format!("{}50{}50{}506020516040511800", call(0), call(32), call(64));
    let (mut world, entry) = fixture(&caller);
    // 读取旧计数器、写入旧值+1、返回旧值。第二与第三次调用有相同的开放
    // abstract Store guard，但具体返回值可以不同，不能复用证书内的复制身份。
    let mut callee = Account::from_hex("5f54806001015f555f5260205ff3", world.fork()).unwrap();
    callee.storage_unknown = true;
    world.insert(address(0x200), callee).unwrap();
    for use_summaries in [false, true] {
        let graph = analyze_world(
            world.clone(),
            entry.clone(),
            ExecutionConfig {
                use_summaries,
                max_work: usize::MAX,
                ..ExecutionConfig::default()
            },
        )
        .unwrap();
        assert_complete(&graph);
        if use_summaries {
            assert!(
                graph.summary_stats().hits > 0,
                "{:?}",
                graph.summary_stats()
            );
        }
        let final_state = graph
            .states()
            .iter()
            .find(|state| {
                state.active().code_address == entry.address
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
