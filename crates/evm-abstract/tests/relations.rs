//! EVM-level relational regressions: feasible paths survive and contradictions do not.
//!
//! These tests inspect native CFG reachability, rather than asking a particular
//! solver or relation representation to report the answer it was constructed with.

use alloy_primitives::{Address, U256, hex};
use evm_abstract::{
    Fork,
    analysis::{self, Config, ExecutionConfig, FrontierReason, Limit, Status, WorldAnalysis},
    bytecode::Program,
    domain::{Profile, Value},
    world::{Account, AddressInput, Entry, EvmEnvironment, World},
};
use revm_bytecode::opcode;
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Assembly {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

struct Fixture {
    code: String,
    labels: BTreeMap<&'static str, usize>,
}

impl Assembly {
    fn op(&mut self, op: u8) -> &mut Self {
        self.bytes.push(op);
        self
    }

    fn push(&mut self, word: u16) -> &mut Self {
        self.bytes.push(opcode::PUSH2);
        self.bytes.extend(word.to_be_bytes());
        self
    }

    fn label(&mut self, name: &'static str) -> &mut Self {
        assert!(self.labels.insert(name, self.bytes.len()).is_none());
        self.op(opcode::JUMPDEST)
    }

    fn jump(&mut self, name: &'static str, conditional: bool) -> &mut Self {
        self.bytes.push(opcode::PUSH2);
        self.fixups.push((self.bytes.len(), name));
        self.bytes.extend([0, 0]);
        self.op(if conditional {
            opcode::JUMPI
        } else {
            opcode::JUMP
        })
    }

    fn guard_eq(&mut self, word: u16, target: &'static str) -> &mut Self {
        self.op(opcode::DUP1)
            .push(word)
            .op(opcode::EQ)
            .jump(target, true)
    }

    fn return_word(&mut self, word: u16) -> &mut Self {
        self.push(word)
            .op(opcode::PUSH0)
            .op(opcode::MSTORE)
            .push(32)
            .op(opcode::PUSH0)
            .op(opcode::RETURN)
    }

    fn call(&mut self, op: u8, target: u16, output: u16, value: u16) -> &mut Self {
        self.push(output)
            .op(opcode::PUSH0)
            .op(opcode::PUSH0)
            .op(opcode::PUSH0);
        if matches!(op, opcode::CALL | opcode::CALLCODE) {
            self.push(value);
        }
        self.push(target).push(60_000).op(op)
    }

    fn finish(mut self) -> Fixture {
        for (offset, label) in self.fixups {
            let pc = u16::try_from(self.labels[&label]).unwrap();
            self.bytes[offset..offset + 2].copy_from_slice(&pc.to_be_bytes());
        }
        Fixture {
            code: hex::encode(self.bytes),
            labels: self.labels,
        }
    }
}

fn analyze(code: &str, profile: Profile, environment: EvmEnvironment) -> analysis::Analysis {
    analysis::analyze_with_environment(
        Program::from_hex_with_fork(code, Fork::Osaka).unwrap(),
        Config {
            domain_profile: profile,
            context_depth: 0,
            ..Config::default()
        },
        environment,
    )
    .unwrap()
}

fn reachable(analysis: &analysis::Analysis, pc: usize) -> bool {
    analysis
        .states()
        .iter()
        .any(|state| analysis.program().blocks()[state.key.basic_block_index].start_pc == pc)
}

fn world_reachable(analysis: &WorldAnalysis, address: Address, pc: usize) -> bool {
    analysis.states().iter().any(|state| {
        state.active().code_address == address
            && state
                .program()
                .and_then(|program| program.blocks().get(state.active().basic_block_index))
                .is_some_and(|block| block.start_pc == pc)
    })
}

fn complete(analysis: &analysis::Analysis) {
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(!analysis.states().is_empty());
    assert!(!analysis.execution().outcomes().is_empty());
}

fn address(word: u16) -> Address {
    Address::from_word(U256::from(word).into())
}

fn world(root: &str, callee: &str) -> (World, Entry) {
    let mut world = World::new(Fork::Osaka, "relational integration fixture");
    for (number, code) in [(0x101, root), (0x200, callee)] {
        let mut account = Account::from_hex(code, Fork::Osaka).unwrap();
        account.balance = Value::constant(U256::from(1_000_000));
        world.insert(address(number), account).unwrap();
    }
    (world, Entry::new(address(0x101)))
}

#[test]
fn repeated_callvalue_guards_cannot_make_one_input_equal_one_and_two() {
    // First true edge establishes x=1; the later x=2 true edge is impossible.
    let code = "3480600114600957005b80600214601257005b00";
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let result = analyze(code, profile, EvmEnvironment::default());
        complete(&result);
        assert!(reachable(&result, 9), "x=1 must remain feasible");
        assert!(
            reachable(&result, 17),
            "x=1 must take the second false edge"
        );
        assert!(
            !reachable(&result, 18),
            "x=1 and x=2 cannot share one execution: {profile:?}"
        );
    }
}

#[test]
fn derived_addition_guard_refines_its_input_instead_of_an_independent_top() {
    let mut code = Assembly::default();
    code.op(opcode::CALLVALUE)
        .op(opcode::DUP1)
        .push(1)
        .op(opcode::ADD)
        .push(2)
        .op(opcode::EQ)
        .jump("derived", true)
        .op(opcode::STOP)
        .label("derived")
        .guard_eq(1, "good")
        .label("bad")
        .op(opcode::STOP)
        .label("good")
        .op(opcode::STOP);
    let fixture = code.finish();
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let result = analyze(&fixture.code, profile, EvmEnvironment::default());
        complete(&result);
        assert!(reachable(&result, fixture.labels["derived"]));
        assert!(reachable(&result, fixture.labels["good"]));
        assert!(
            !reachable(&result, fixture.labels["bad"]),
            "x+1=2 implies x=1: {profile:?}"
        );
    }
}

#[test]
fn symbolic_addition_uses_evm_word_wraparound_and_preserves_the_wrapped_path() {
    let mut code = Assembly::default();
    code.op(opcode::CALLVALUE)
        .op(opcode::DUP1)
        .push(1)
        .op(opcode::ADD)
        .op(opcode::ISZERO)
        .jump("wrapped", true)
        .op(opcode::STOP)
        .label("wrapped")
        .op(opcode::ISZERO)
        .jump("bad", true)
        .label("good")
        .op(opcode::STOP)
        .label("bad")
        .op(opcode::STOP);
    let fixture = code.finish();
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let result = analyze(&fixture.code, profile, EvmEnvironment::default());
        complete(&result);
        assert!(
            reachable(&result, fixture.labels["wrapped"]),
            "x=U256::MAX is valid"
        );
        assert!(reachable(&result, fixture.labels["good"]));
        assert!(
            !reachable(&result, fixture.labels["bad"]),
            "x+1=0 cannot also have x=0"
        );
    }
}

#[test]
fn false_edges_exclude_the_same_equality_on_later_blocks() {
    let mut code = Assembly::default();
    code.op(opcode::CALLVALUE)
        .guard_eq(1, "one")
        .guard_eq(1, "bad")
        .label("not_one")
        .op(opcode::STOP)
        .label("one")
        .op(opcode::STOP)
        .label("bad")
        .op(opcode::STOP);
    let fixture = code.finish();
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let result = analyze(&fixture.code, profile, EvmEnvironment::default());
        complete(&result);
        assert!(reachable(&result, fixture.labels["one"]));
        assert!(reachable(&result, fixture.labels["not_one"]));
        assert!(
            !reachable(&result, fixture.labels["bad"]),
            "false guard means x!=1"
        );
    }
}

#[test]
fn joined_paths_keep_the_union_of_one_and_two_and_exclude_a_third_value() {
    let mut code = Assembly::default();
    code.op(opcode::CALLVALUE)
        .op(opcode::PUSH0)
        .op(opcode::CALLDATALOAD)
        .jump("left", true)
        .guard_eq(2, "right_ok")
        .op(opcode::STOP)
        .label("left")
        .guard_eq(1, "left_ok")
        .op(opcode::STOP)
        .label("right_ok")
        .jump("merge", false)
        .label("left_ok")
        .jump("merge", false)
        .label("merge")
        .guard_eq(1, "one")
        .guard_eq(2, "two")
        .label("bad")
        .op(opcode::STOP)
        .label("one")
        .op(opcode::STOP)
        .label("two")
        .op(opcode::STOP);
    let fixture = code.finish();
    for profile in [Profile::Product, Profile::ConstantsOnly] {
        let result = analyze(&fixture.code, profile, EvmEnvironment::default());
        complete(&result);
        assert_eq!(
            result
                .states()
                .iter()
                .filter(
                    |state| result.program().blocks()[state.key.basic_block_index].start_pc
                        == fixture.labels["merge"]
                )
                .count(),
            1,
            "context_depth=0 joins the same machine key"
        );
        for branch in ["left_ok", "right_ok", "one", "two"] {
            assert!(
                reachable(&result, fixture.labels[branch]),
                "lost feasible {branch}: {profile:?}"
            );
        }
        assert!(
            !reachable(&result, fixture.labels["bad"]),
            "join is x=1 OR x=2, not AND or an unconstrained x"
        );
    }
}

fn bounded_loop() -> Fixture {
    let mut code = Assembly::default();
    code.op(opcode::CALLVALUE)
        .op(opcode::PUSH0)
        .label("head")
        .op(opcode::DUP1)
        .push(4)
        .op(opcode::SWAP1)
        .op(opcode::LT)
        .jump("body", true)
        .label("exit")
        .op(opcode::POP)
        .op(opcode::CALLVALUE)
        .op(opcode::EQ)
        .op(opcode::STOP)
        .label("body")
        .push(1)
        .op(opcode::ADD)
        .jump("head", false);
    code.finish()
}

#[test]
fn loop_fixed_point_retains_immutable_input_identity_without_unbounded_unrolling() {
    let fixture = bounded_loop();
    let result = analyze(&fixture.code, Profile::Product, EvmEnvironment::default());
    complete(&result);
    assert!(reachable(&result, fixture.labels["body"]));
    assert!(reachable(&result, fixture.labels["exit"]));
    assert!(
        result.transfers() <= 256,
        "finite counter loop needs widening or a finite fixed point"
    );
    for state in result.states().iter().filter(|state| {
        result.program().blocks()[state.key.basic_block_index].start_pc == fixture.labels["exit"]
    }) {
        assert_eq!(
            state.exit_stack.last().unwrap().singleton(),
            Some(U256::from(1)),
            "re-read immutable CALLVALUE equals its pre-loop input"
        );
    }
}

#[test]
fn a_loop_cut_short_by_the_shared_transfer_budget_remains_typed_incomplete() {
    let fixture = bounded_loop();
    let result = analysis::analyze_with_environment(
        Program::from_hex(&fixture.code).unwrap(),
        Config {
            context_depth: 0,
            max_transfers: 1,
            ..Config::default()
        },
        EvmEnvironment::default(),
    )
    .unwrap();
    assert_eq!(result.status(), Status::Incomplete);
    assert!(
        result
            .frontiers()
            .iter()
            .any(|frontier| frontier.limit == Limit::Transfers)
    );
    assert!(
        !result.states().is_empty(),
        "execution started before the transfer limit"
    );
}

fn origin_callee() -> Fixture {
    let mut code = Assembly::default();
    code.op(opcode::ORIGIN)
        .push(2)
        .op(opcode::EQ)
        .jump("origin_two", true)
        .label("origin_other")
        .op(opcode::STOP)
        .label("origin_two")
        .op(opcode::STOP);
    code.finish()
}

#[test]
fn delegatecall_shares_default_origin_constraints_but_not_an_independent_origin() {
    let mut root = Assembly::default();
    root.op(opcode::CALLER)
        .guard_eq(1, "delegate")
        .op(opcode::STOP)
        .label("delegate")
        .op(opcode::POP)
        .call(opcode::DELEGATECALL, 0x200, 0, 0)
        .op(opcode::POP)
        .op(opcode::STOP);
    let root = root.finish();
    let child = origin_callee();
    for independent in [false, true] {
        let (world, mut entry) = world(&root.code, &child.code);
        if independent {
            entry.environment.origin = Some(AddressInput::unknown_origin());
        }
        let result = analysis::analyze_world(world, entry, ExecutionConfig::default()).unwrap();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        assert!(world_reachable(
            &result,
            address(0x200),
            child.labels["origin_other"]
        ));
        assert_eq!(
            world_reachable(&result, address(0x200), child.labels["origin_two"]),
            independent,
            "default caller=1 implies origin=1 in every frame; independent origin may be 2"
        );
    }
}

#[test]
fn a_child_callvalue_is_not_the_parent_callvalue_variable() {
    let mut root = Assembly::default();
    root.op(opcode::CALLVALUE)
        .guard_eq(1, "call")
        .op(opcode::STOP)
        .label("call")
        .op(opcode::POP)
        .call(opcode::CALL, 0x200, 0, 2)
        .op(opcode::POP)
        .op(opcode::STOP);
    let root = root.finish();
    let mut child = Assembly::default();
    child
        .op(opcode::CALLVALUE)
        .push(2)
        .op(opcode::EQ)
        .jump("two", true)
        .label("bad")
        .op(opcode::STOP)
        .label("two")
        .op(opcode::STOP);
    let child = child.finish();
    let (world, entry) = world(&root.code, &child.code);
    let result = analysis::analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(
        world_reachable(&result, address(0x200), child.labels["two"]),
        "parent value=1 does not conflict with CALL's child value=2"
    );
    assert!(!world_reachable(
        &result,
        address(0x200),
        child.labels["bad"]
    ));
}

fn numeric_json(value: &mut Json) {
    match value {
        Json::Array(values) => values.iter_mut().for_each(numeric_json),
        Json::Object(fields) => {
            // Compare numeric coverage and world effects, not diagnostic source
            // identities or expression arena numbering from separate analyses.
            if fields.contains_key("known_bits") || fields.contains_key("Constants") {
                fields.retain(|name, _| {
                    matches!(
                        name.as_str(),
                        "Constants" | "known_bits" | "interval" | "congruence" | "nonzero"
                    )
                });
            }
            fields.values_mut().for_each(numeric_json);
        }
        _ => {}
    }
}

fn outcome_relations(result: &WorldAnalysis) -> BTreeSet<String> {
    result
        .outcomes()
        .iter()
        .map(|outcome| {
            let mut value =
                serde_json::to_value((outcome.kind, &outcome.data, &outcome.store)).unwrap();
            numeric_json(&mut value);
            serde_json::to_string(&value).unwrap()
        })
        .collect()
}

#[test]
fn summary_reuse_preserves_parent_constraints_and_numeric_outcome_parity() {
    let mut root = Assembly::default();
    root.op(opcode::CALLER)
        .guard_eq(1, "calls")
        .op(opcode::STOP)
        .label("calls")
        .op(opcode::POP)
        .call(opcode::STATICCALL, 0x200, 32, 0)
        .op(opcode::POP)
        .call(opcode::STATICCALL, 0x200, 32, 0)
        .op(opcode::POP)
        .push(32)
        .op(opcode::PUSH0)
        .op(opcode::RETURN);
    let root = root.finish();
    let mut child = Assembly::default();
    child
        .op(opcode::ORIGIN)
        .push(1)
        .op(opcode::EQ)
        .jump("good", true)
        .label("bad")
        .return_word(99)
        .label("good")
        .return_word(1);
    let child = child.finish();
    let (world, entry) = world(&root.code, &child.code);
    let mut outcomes = Vec::new();
    for enabled in [true, false] {
        let result = analysis::analyze_world(
            world.clone(),
            entry.clone(),
            ExecutionConfig {
                use_summaries: enabled,
                analysis: Config {
                    context_depth: 0,
                    ..Config::default()
                },
                ..ExecutionConfig::default()
            },
        )
        .unwrap();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        assert!(world_reachable(
            &result,
            address(0x200),
            child.labels["good"]
        ));
        assert!(
            !world_reachable(&result, address(0x200), child.labels["bad"]),
            "callee ORIGIN is constrained by parent's default caller"
        );
        if enabled {
            assert!(
                result.summary_stats().hits > 0,
                "exercise actual certificate reuse: {:?}",
                result.summary_stats()
            );
        } else {
            assert_eq!(result.summary_stats().hits, 0);
        }
        outcomes.push(outcome_relations(&result));
    }
    assert_eq!(outcomes[0], outcomes[1]);
}

#[test]
fn summary_replay_renames_fresh_gas_words_in_callee_memory() {
    let mut root = Assembly::default();
    root.call(opcode::STATICCALL, 0x200, 32, 0)
        .op(opcode::POP)
        .call(opcode::STATICCALL, 0x200, 32, 0)
        .op(opcode::POP)
        .push(32)
        .op(opcode::PUSH0)
        .op(opcode::RETURN);
    let root = root.finish();
    // GAS is a mutable runtime observation, not an immutable environment input.
    // Inspect the callee before return joins can erase the word's expression.
    let (world, entry) = world(&root.code, "5a5f5260205ff3");
    let result = analysis::analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(
        result.summary_stats().hits > 0,
        "{:?}",
        result.summary_stats()
    );
    let record = result
        .summaries()
        .iter()
        .find(|record| !record.reused_at.is_empty())
        .expect("a certified call was actually replayed");
    let domain = evm_abstract::domain::Domain::from_spec(result.domain_spec());
    let original = result.states()[record.source_state]
        .exit
        .as_ref()
        .unwrap()
        .active()
        .memory
        .read_word(&Value::constant(U256::ZERO), domain);
    let original_leaves = original
        .expression()
        .expect("unknown GAS word remains symbolic")
        .fresh_leaves();
    assert!(!original_leaves.is_empty(), "GAS is a fresh runtime value");
    for &replayed in &record.reused_at {
        let state = &result.states()[replayed];
        assert_eq!(state.active().code_address, address(0x200));
        let imported = state
            .exit
            .as_ref()
            .unwrap()
            .active()
            .memory
            .read_word(&Value::constant(U256::ZERO), domain);
        assert_eq!(
            original.numeric(),
            imported.numeric(),
            "replay preserves numeric coverage"
        );
        let imported_leaves = imported
            .expression()
            .expect("replayed GAS remains symbolic")
            .fresh_leaves();
        assert!(!imported_leaves.is_empty());
        assert!(
            original_leaves.is_disjoint(&imported_leaves),
            "independent executions must not share their runtime GAS leaf"
        );
    }
}

fn overwritten_read(memory: bool) -> Fixture {
    let mut code = Assembly::default();
    if memory {
        // Memory starts as zero, so first install an unknown word. The guarded
        // historical MLOAD is then overwritten, while its stack copy is retained.
        code.op(opcode::CALLVALUE)
            .op(opcode::PUSH0)
            .op(opcode::MSTORE);
    }
    let read = if memory { opcode::MLOAD } else { opcode::SLOAD };
    let write = if memory {
        opcode::MSTORE
    } else {
        opcode::SSTORE
    };
    code.op(opcode::PUSH0)
        .op(read)
        .guard_eq(1, "write")
        .op(opcode::STOP)
        .label("write")
        .push(2)
        .op(opcode::PUSH0)
        .op(write)
        .op(opcode::PUSH0)
        .op(read)
        .op(opcode::EQ)
        .jump("bad", true)
        .label("good")
        .op(opcode::STOP)
        .label("bad")
        .op(opcode::STOP);
    code.finish()
}

#[test]
fn storage_write_does_not_rebind_the_symbol_of_a_historical_sload() {
    let fixture = overwritten_read(false);
    let result = analyze(&fixture.code, Profile::Product, EvmEnvironment::default());
    complete(&result);
    assert!(reachable(&result, fixture.labels["write"]));
    assert!(
        reachable(&result, fixture.labels["good"]),
        "old slot value=1 and new slot value=2 are compatible"
    );
    assert!(
        !reachable(&result, fixture.labels["bad"]),
        "the two read versions are unequal"
    );
}

#[test]
fn memory_write_does_not_rebind_the_symbol_of_a_historical_mload() {
    let fixture = overwritten_read(true);
    let result = analyze(&fixture.code, Profile::Product, EvmEnvironment::default());
    complete(&result);
    assert!(reachable(&result, fixture.labels["write"]));
    assert!(reachable(&result, fixture.labels["good"]));
    assert!(
        !reachable(&result, fixture.labels["bad"]),
        "old word=1 is not the new word=2"
    );
}

#[test]
fn an_unknown_storage_write_may_alias_a_previously_read_slot() {
    let mut code = Assembly::default();
    code.op(opcode::PUSH0)
        .op(opcode::SLOAD)
        .guard_eq(1, "write")
        .op(opcode::STOP)
        .label("write")
        .op(opcode::POP)
        .push(2)
        .op(opcode::PUSH0)
        .op(opcode::CALLDATALOAD)
        .op(opcode::SSTORE)
        .op(opcode::PUSH0)
        .op(opcode::SLOAD)
        .guard_eq(2, "changed")
        .guard_eq(1, "unchanged")
        .op(opcode::STOP)
        .label("changed")
        .op(opcode::STOP)
        .label("unchanged")
        .op(opcode::STOP);
    let fixture = code.finish();
    let result = analyze(&fixture.code, Profile::Product, EvmEnvironment::default());
    complete(&result);
    assert!(
        reachable(&result, fixture.labels["changed"]),
        "unknown write may hit slot zero"
    );
    assert!(
        reachable(&result, fixture.labels["unchanged"]),
        "unknown write may hit another slot"
    );
}

#[test]
fn exhausted_shared_work_never_manufactures_a_complete_empty_graph() {
    let code = "3480600114600957005b80600214601257005b00";
    let (world, entry) = world(code, "00");
    let result = analysis::analyze_world(
        world,
        entry,
        ExecutionConfig {
            max_work: 1,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(result.status(), Status::Incomplete);
    assert!(
        result
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Work)
    );
}

fn one_symbolic_guard() -> Fixture {
    let mut code = Assembly::default();
    code.op(opcode::CALLVALUE)
        .push(1)
        .op(opcode::ADD)
        .push(2)
        .op(opcode::EQ)
        .jump("true", true)
        .label("false")
        .op(opcode::STOP)
        .label("true")
        .op(opcode::STOP);
    code.finish()
}

fn limited(
    fixture: &Fixture,
    limits: evm_abstract::domain::relational::RelationLimits,
) -> analysis::Analysis {
    analysis::analyze_with_environment(
        Program::from_hex(&fixture.code).unwrap(),
        Config {
            context_depth: 0,
            relations: limits,
            ..Config::default()
        },
        EvmEnvironment::default(),
    )
    .unwrap()
}

fn retained_unknown_branches(result: &analysis::Analysis, fixture: &Fixture) {
    assert_eq!(
        result.status(),
        Status::Incomplete,
        "query did not establish closure"
    );
    assert!(
        reachable(result, fixture.labels["true"]),
        "the true side has a concrete witness"
    );
    assert!(
        reachable(result, fixture.labels["false"]),
        "the false side has a concrete witness"
    );
}

#[test]
fn native_smt_resource_exhaustion_preserves_both_feasible_branches() {
    use evm_abstract::domain::relational::{QueryReason, RelationLimits};
    let fixture = one_symbolic_guard();
    let result = limited(
        &fixture,
        RelationLimits {
            rlimit: 1,
            ..RelationLimits::default()
        },
    );
    retained_unknown_branches(&result, &fixture);
    assert!(
        result.execution().frontiers().iter().any(|frontier| {
            frontier.reason == FrontierReason::Relations(QueryReason::ResourceLimit)
        }),
        "native rlimit exhaustion must remain typed, never UNSAT"
    );
}

#[test]
fn symbolic_node_and_depth_limits_preserve_both_feasible_branches() {
    use evm_abstract::domain::relational::{QueryReason, RelationLimits};
    let fixture = one_symbolic_guard();
    for limits in [
        RelationLimits {
            max_nodes: 1,
            ..RelationLimits::default()
        },
        RelationLimits {
            max_depth: 1,
            ..RelationLimits::default()
        },
    ] {
        let result = limited(&fixture, limits);
        retained_unknown_branches(&result, &fixture);
        assert!(
            result.execution().frontiers().iter().any(|frontier| {
                frontier.reason == FrontierReason::Relations(QueryReason::ExpressionLimit)
            }),
            "expression exhaustion must retain its reason"
        );
    }
}

#[test]
fn a_constraint_limit_is_unknown_and_does_not_drop_a_feasible_side() {
    use evm_abstract::domain::relational::{QueryReason, RelationLimits};
    // Two independent non-singleton conditions need two retained assumptions.
    // This does not rely on how many redundant scalar facts observe() emits.
    // Second true: CALLVALUE=1 and calldata word=1; second false: 1 and 0.
    let mut code = Assembly::default();
    code.op(opcode::CALLVALUE)
        .jump("second", true)
        .label("first_false")
        .op(opcode::STOP)
        .label("second")
        .op(opcode::PUSH0)
        .op(opcode::CALLDATALOAD)
        .jump("true", true)
        .label("false")
        .op(opcode::STOP)
        .label("true")
        .op(opcode::STOP);
    let fixture = code.finish();
    let result = limited(
        &fixture,
        RelationLimits {
            max_constraints: 1,
            ..RelationLimits::default()
        },
    );
    retained_unknown_branches(&result, &fixture);
    assert!(
        reachable(&result, fixture.labels["first_false"]),
        "CALLVALUE=0 is feasible before the second guard"
    );
    assert!(
        result.execution().frontiers().iter().any(|frontier| {
            frontier.reason == FrontierReason::Relations(QueryReason::ConstraintLimit)
        }),
        "the exhausted retained-atom limit must be explicit"
    );
}

#[test]
fn disabling_relations_restores_the_broader_baseline_cfg() {
    use evm_abstract::domain::relational::RelationLimits;
    let code = "3480600114600957005b80600214601257005b00";
    let result = analysis::analyze_with_environment(
        Program::from_hex(code).unwrap(),
        Config {
            context_depth: 0,
            relations: RelationLimits {
                enabled: false,
                ..RelationLimits::default()
            },
            ..Config::default()
        },
        EvmEnvironment::default(),
    )
    .unwrap();
    complete(&result);
    assert!(
        reachable(&result, 18),
        "the ablation must not quietly retain relational guard pruning"
    );
}
