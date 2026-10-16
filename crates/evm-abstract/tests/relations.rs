//! EVM-level relational regressions: feasible paths survive and contradictions do not.
//!
//! These tests inspect CFG reachability, concrete controls and local guarantees.

use alloy_primitives::{Address, U256, hex};
use evm_abstract::{
    Fork,
    analysis::{self, Config, ExecutionConfig, FrontierReason, Limit, Status, WorldAnalysis},
    bytecode::Program,
    domain::{AbstractValue, Profile},
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
        account.balance = AbstractValue::constant(U256::from(1_000_000));
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

fn alpha_json(value: &mut Json, leaves: &mut BTreeMap<(String, u64), u64>) {
    match value {
        Json::Array(values) => {
            for value in values {
                alpha_json(value, leaves);
            }
        }
        Json::Object(fields) => {
            // Preserve every numeric/provenance field and all symbolic structure.
            // Only process-private Fresh names differ between independent runs.
            if let Some(scope) = fields
                .get("identity_scope")
                .and_then(Json::as_str)
                .map(str::to_owned)
            {
                for node in fields.get_mut("nodes").unwrap().as_array_mut().unwrap() {
                    if let Some(token) = node.get_mut("Fresh") {
                        let next = u64::try_from(leaves.len()).unwrap();
                        let renamed = *leaves
                            .entry((scope.clone(), token.as_u64().unwrap()))
                            .or_insert(next);
                        *token = Json::from(renamed);
                    }
                }
            }
            for value in fields.values_mut() {
                alpha_json(value, leaves);
            }
        }
        _ => {}
    }
}

fn outcome_relations(result: &WorldAnalysis) -> BTreeSet<String> {
    // One mapping covers every graph and outcome, preserving shared leaves.
    let mut leaves = BTreeMap::new();
    result
        .outcomes()
        .iter()
        .map(|outcome| {
            let mut value =
                serde_json::to_value((outcome.kind, &outcome.data, &outcome.store)).unwrap();
            alpha_json(&mut value, &mut leaves);
            serde_json::to_string(&value).unwrap()
        })
        .collect()
}

#[test]
fn summary_reuse_preserves_parent_constraints_and_numeric_outcome_parity() {
    use evm_abstract::domain::symbolic::{ExprId, ExprLimits, SlotStateId, SourceId};
    let canonical = |mut value: Json| {
        alpha_json(&mut value, &mut BTreeMap::new());
        value
    };
    let a = ExprId::fresh().unwrap();
    let b = ExprId::fresh().unwrap();
    let c = ExprId::fresh().unwrap();
    let d = ExprId::fresh().unwrap();
    assert_eq!(
        canonical(serde_json::to_value([&a, &a, &b]).unwrap()),
        canonical(serde_json::to_value([&c, &c, &d]).unwrap())
    );
    assert_ne!(
        canonical(serde_json::to_value([&a, &a, &b]).unwrap()),
        canonical(serde_json::to_value([&c, &d, &d]).unwrap())
    );
    let bounded = AbstractValue::unsigned_range(U256::from(1), U256::from(3))
        .unwrap()
        .with_expression(a.clone());
    let expanded = AbstractValue::unsigned_range(U256::from(1), U256::from(4))
        .unwrap()
        .with_expression(c.clone());
    assert_ne!(
        canonical(serde_json::to_value(&bounded).unwrap()),
        canonical(serde_json::to_value(&expanded).unwrap())
    );
    let sourced = AbstractValue::constant(U256::from(1)).with_expression(a.clone());
    assert_ne!(
        canonical(serde_json::to_value(&sourced).unwrap()),
        canonical(
            serde_json::to_value(
                sourced
                    .clone()
                    .with_origin(evm_abstract::domain::provenance::Origin::Storage)
            )
            .unwrap()
        )
    );
    let caller = evm_abstract::domain::identity::Symbol::Caller;
    assert_ne!(
        canonical(serde_json::to_value(ExprId::input(1, caller)).unwrap()),
        canonical(serde_json::to_value(ExprId::input(2, caller)).unwrap())
    );
    let read = |namespace| {
        ExprId::slot_read(
            SlotStateId::input(SourceId::scoped(a.clone(), namespace)),
            ExprId::constant(U256::ZERO),
            ExprLimits::default(),
        )
        .unwrap()
    };
    assert_ne!(
        canonical(serde_json::to_value(read(U256::from(1))).unwrap()),
        canonical(serde_json::to_value(read(U256::from(2))).unwrap())
    );
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
    let mut fixed_inputs = Vec::new();
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
        let initial = &result.states()[0].entry.call_stack.root().state;
        fixed_inputs.push(
            serde_json::to_value((
                &initial.calldata,
                initial.call_value.expression(),
                &initial.saved_store,
            ))
            .unwrap(),
        );
        for state in result.states() {
            let root = &state.entry.call_stack.root().state;
            assert_eq!(root.calldata.state_root(), initial.calldata.state_root());
            assert_eq!(
                root.calldata.len().expression(),
                initial.calldata.len().expression()
            );
            assert_eq!(
                root.call_value.expression(),
                initial.call_value.expression()
            );
        }
        for outcome in result.outcomes() {
            for owner in world.accounts().keys() {
                assert_eq!(
                    outcome.store.slot_root(*owner, false),
                    initial.saved_store.state().slot_root(*owner, false)
                );
            }
        }
        outcomes.push(outcome_relations(&result));
    }
    assert_eq!(fixed_inputs[0], fixed_inputs[1]);
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
        .read_word(&AbstractValue::constant(U256::ZERO), domain);
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
            .read_word(&AbstractValue::constant(U256::ZERO), domain);
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
fn local_work_exhaustion_retains_guards_and_recovers_exact_with_the_default_policy() {
    use evm_abstract::domain::{
        relational::{CheckResult, QueryReason, RelationLimits, RelationState, ValueQuery},
        symbolic::ExprId,
    };
    let policy = RelationLimits::default();
    let input = ExprId::fresh().unwrap();
    let derived = ExprId::operation(
        opcode::ADD,
        &[input.clone(), ExprId::constant(U256::ONE)],
        policy.expressions(),
    )
    .unwrap();
    let guard = ExprId::operation(
        opcode::EQ,
        &[derived, ExprId::constant(U256::from(2))],
        policy.expressions(),
    )
    .unwrap();
    let mut relations = RelationState::default();
    assert_eq!(
        relations.assume(&guard, true, &policy),
        CheckResult::MayBeFeasible
    );
    let before = relations.clone();
    // The obsolete native-fuel fixture now bounds actual local steps. The
    // complete retained graph fits; importing and validating it needs more work.
    let bounded = RelationLimits {
        max_nodes: relations.work_size(),
        ..policy
    };
    assert!(guard.nodes() <= bounded.max_nodes);
    assert!(guard.depth() <= bounded.max_depth);
    assert_eq!(
        relations.local_value(&input, &bounded),
        ValueQuery::Undetermined(QueryReason::ExpressionLimit)
    );
    assert_eq!(relations, before, "interruption must preserve the guard");
    assert_eq!(
        relations.local_value(&input, &policy),
        ValueQuery::Exact(U256::ONE)
    );
    assert_eq!(
        relations.can_equal(&input, U256::ZERO, &policy),
        CheckResult::ProvenImpossible
    );
    let fixture = one_symbolic_guard();
    let result = limited(&fixture, policy);
    complete(&result);
    assert!(reachable(&result, fixture.labels["true"]));
    assert!(reachable(&result, fixture.labels["false"]));
}

#[test]
fn initial_symbolic_store_uses_the_configured_policy_and_retains_failure() {
    use evm_abstract::domain::{Domain, relational::QueryReason};
    let owner = address(0x101);
    let mut world = World::new(Fork::Osaka, "initial state construction limit");
    let mut account = Account::from_hex("00", Fork::Osaka).unwrap();
    account.storage = (0..65)
        .map(|i| (U256::from(i), AbstractValue::constant(U256::from(i + 1))))
        .collect();
    world.insert(owner, account).unwrap();
    let entry = Entry::new(owner);
    let limited =
        analysis::analyze_world(world.clone(), entry.clone(), ExecutionConfig::default()).unwrap();
    assert_eq!(limited.status(), Status::Incomplete);
    assert!(limited.outcomes().is_empty());
    assert!(limited.frontiers().iter().any(|frontier| {
        frontier.reason == FrontierReason::Relations(QueryReason::ExpressionLimit)
    }));
    let mut config = ExecutionConfig::default();
    config.analysis.relations.max_depth = 128;
    let domain = Domain::from_spec(
        Domain::default()
            .spec()
            .with_relations(config.analysis.relations),
    );
    let result = analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(!result.outcomes().is_empty());
    for outcome in result.outcomes() {
        assert_eq!(
            outcome
                .store
                .read_checked(owner, &AbstractValue::constant(U256::from(64)), domain)
                .unwrap()
                .singleton(),
            Some(U256::from(65))
        );
    }
}

#[test]
fn cancelled_range_analysis_can_be_retried_without_losing_either_outcome() {
    use evm_abstract::analysis::{ControlledAnalysisError, control::Control, progress::Observer};
    let (world, mut entry) = world("5f345f3e00", "00");
    entry.environment.value = AbstractValue::top();
    // Replace the removed fuel knob with an actual cancellation boundary. Both
    // the retry and concrete controls keep the original execution defaults.
    let control = Control::new(Observer::default());
    control.cancellation().cancel();
    assert!(matches!(
        analysis::analyze_world_with_control(
            world.clone(),
            entry.clone(),
            ExecutionConfig::default(),
            &control
        ),
        Err(ControlledAnalysisError::Cancelled)
    ));
    let result =
        analysis::analyze_world(world.clone(), entry.clone(), ExecutionConfig::default()).unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(result.frontiers().is_empty());
    for kind in [
        analysis::OutcomeKind::Return,
        analysis::OutcomeKind::Failure,
    ] {
        assert!(result.outcomes().iter().any(|outcome| outcome.kind == kind));
    }
    for (size, returns) in [(0u64, true), (1, false)] {
        let mut concrete = entry.clone();
        concrete.environment.value = AbstractValue::constant(U256::from(size));
        let control =
            analysis::analyze_world(world.clone(), concrete, ExecutionConfig::default()).unwrap();
        assert_eq!(control.status(), Status::Converged);
        assert_eq!(
            control
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == analysis::OutcomeKind::Return),
            returns,
            "empty returndata permits size zero and rejects size one"
        );
        assert!(
            control
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == analysis::OutcomeKind::Failure)
        );
    }
}

#[test]
fn optional_state_read_projection_reports_precision_loss_without_unexpanded_paths() {
    use evm_abstract::domain::relational::{QueryReason, ValueQuery};
    let (world, entry) = world("5f3500", "00");
    let mut config = ExecutionConfig::default();
    // This replaces only the obsolete native-fuel fixture. Sixty-four nodes
    // retain the read graph, but cannot pay the real 32-byte local projection.
    config.analysis.relations.max_nodes = 64;
    let analysis = analysis::analyze_world(world.clone(), entry.clone(), config.clone()).unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(analysis.frontiers().is_empty());
    assert!(
        analysis
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&2))
    );
    assert!(analysis.diagnostics().iter().any(|diagnostic| {
        matches!(
            diagnostic.kind,
            analysis::DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit)
        )
    }));
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == analysis::OutcomeKind::Return)
    );
    let loaded = &analysis
        .states()
        .iter()
        .find(|state| state.executed_pcs.contains(&1))
        .unwrap()
        .exit_stack[0];
    assert!(loaded.contains(U256::ZERO));
    assert!(loaded.contains(U256::MAX));
    let expression = loaded
        .expression()
        .expect("interruption retains the input read");
    assert!(expression.nodes() <= config.analysis.relations.max_nodes);
    assert!(expression.depth() <= config.analysis.relations.max_depth);
    assert_eq!(
        evm_abstract::domain::relational::RelationState::default()
            .local_value(expression, &config.analysis.relations),
        ValueQuery::Undetermined(QueryReason::ExpressionLimit)
    );
    let ValueQuery::Bounds(bounds) = evm_abstract::domain::relational::RelationState::default()
        .local_value(expression, &Default::default())
    else {
        panic!("the default local policy must complete the same read");
    };
    for value in [U256::ZERO, U256::MAX] {
        assert!(bounds.contains(value), "local read bounds excluded {value}");
    }
    let mut concrete = entry;
    concrete.environment.calldata = evm_abstract::world::ByteArray::exact(&[7; 32]);
    let exact = analysis::analyze_world(world, concrete, config).unwrap();
    assert_eq!(exact.status(), Status::Converged);
    assert_eq!(
        exact
            .states()
            .iter()
            .find(|state| state.executed_pcs.contains(&1))
            .unwrap()
            .exit_stack[0]
            .singleton(),
        Some(U256::from_be_bytes([7; 32]))
    );
}

#[test]
fn symbolic_node_and_depth_limits_preserve_both_feasible_branches() {
    use evm_abstract::domain::relational::{QueryReason, RelationLimits};
    let fixture = one_symbolic_guard();
    for limits in [
        RelationLimits {
            // Two nodes retain the initial account-source root; the derived
            // branch expression still exceeds this limit.
            max_nodes: 2,
            ..RelationLimits::default()
        },
        RelationLimits {
            max_depth: 2,
            ..RelationLimits::default()
        },
    ] {
        let result = analysis::analyze_with_environment(
            Program::from_hex(&fixture.code).unwrap(),
            Config {
                context_depth: 0,
                relations: limits,
                ..Config::default()
            },
            EvmEnvironment {
                // Calldata is unused; keep the initial root within the tiny
                // policy so the derived CALLVALUE branch is what exhausts it.
                calldata: evm_abstract::world::ByteArray::empty(),
                ..EvmEnvironment::default()
            },
        )
        .unwrap();
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
fn uncompared_lazy_hash_keeps_concrete_hashes_without_a_precision_failure() {
    use evm_abstract::{
        analysis::DiagnosticKind,
        domain::{Domain, relational::ValueQuery},
        world::ByteArray,
    };
    // A partial overwrite of a 128-byte symbolic copy retains a content term.
    // Its ordinary unknown value is not an unsupported-provider diagnostic.
    let result = analysis::analyze_with_environment(
        Program::from_hex("60805f5f3760ff5f5360805f205000").unwrap(),
        Config::default(),
        EvmEnvironment::default(),
    )
    .unwrap();
    complete(&result);
    assert!(result.execution().diagnostics().iter().all(|diagnostic| {
        !matches!(diagnostic.kind, DiagnosticKind::RelationalPrecisionLost(_))
    }));
    // No memory write follows KECCAK256, so the actual terminal memory is its
    // input snapshot even though POP removes the intermediate stack result.
    let payload = result
        .execution()
        .states()
        .iter()
        .find_map(|state| state.exit.as_ref())
        .unwrap();
    let domain = Domain::from_spec(result.execution().domain_spec());
    let materialize_limit = ExecutionConfig::default().max_memory_bytes;
    let input = payload
        .active()
        .memory
        .slice(
            &AbstractValue::constant(U256::ZERO),
            &AbstractValue::constant(U256::from(128)),
            materialize_limit,
            domain,
        )
        .unwrap();
    let hash = input.keccak(domain, materialize_limit).unwrap();
    let answer = payload
        .relations
        .local_value(hash.expression().unwrap(), &domain.spec().relations());
    let ValueQuery::Bounds(bounds) = answer else {
        panic!("unknown content should retain completed local bounds: {answer:?}");
    };
    for fill in [0u8, 7] {
        let mut bytes = [fill; 128];
        bytes[0] = 255;
        let expected = U256::from_be_bytes(alloy_primitives::keccak256(bytes).0);
        assert!(hash.contains(expected));
        assert!(
            bounds.contains(expected),
            "local bounds excluded concrete hash {expected}"
        );
        let exact = ByteArray::exact(&bytes)
            .keccak(domain, materialize_limit)
            .unwrap();
        assert_eq!(exact.singleton(), Some(expected));
        assert_eq!(
            evm_abstract::domain::relational::RelationState::default()
                .local_value(exact.expression().unwrap(), &domain.spec().relations()),
            ValueQuery::Exact(expected)
        );
    }
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
