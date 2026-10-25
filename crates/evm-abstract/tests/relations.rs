//! EVM-level relational regressions: feasible paths survive and contradictions do not.
//!
//! These tests inspect CFG reachability, concrete controls and local guarantees.
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisInput, EvmAnalysis};
use evm_abstract::domain::{EvmNumericAlgebra, ValueAlgebra, WordElement};

#[path = "support/native.rs"]
mod native;

use alloy_primitives::{Address, U256, hex};
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork,
    analysis::{self, Config, EvmWorldAnalysis, ExecutionConfig, FrontierReason, Limit, Status},
    bytecode::Program,
    domain::EvmValue,
    world::{AddressInput, EvmAccount, EvmEntry, EvmEnvironmentValues, EvmWorld},
};
use revm_bytecode::opcode;
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};

#[path = "support/assembly.rs"]
mod bytecode;

#[derive(Default)]
struct Assembly {
    raw: bytecode::Assembly,
}

struct Fixture {
    code: String,
    labels: BTreeMap<&'static str, usize>,
}

impl Assembly {
    fn op(&mut self, op: u8) -> &mut Self {
        self.raw.op(op);
        self
    }

    fn push(&mut self, value: u16) -> &mut Self {
        self.raw.push(value);
        self
    }

    fn label(&mut self, name: &'static str) -> &mut Self {
        self.raw.label(name);
        self
    }

    fn jump(&mut self, name: &'static str, conditional: bool) -> &mut Self {
        self.raw.jump(name, conditional);
        self
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

    fn finish(self) -> Fixture {
        let assembled = self.raw.finish();
        Fixture {
            code: hex::encode(assembled.bytes),
            labels: assembled.labels,
        }
    }
}

fn relations_case<D: EvmNumericAlgebra>(
    values: &ValueAlgebra<D>,
    code: &str,
    environment: EvmEnvironmentValues<D::Element>,
) -> EvmAnalysis<D::Element> {
    let input = AnalysisInput::from_program(
        Program::from_hex_with_fork(code, Fork::Osaka).unwrap(),
        environment,
    )
    .unwrap();
    let graph = native::run(
        values,
        input,
        ExecutionConfig {
            analysis: Config {
                context_depth: 0,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .unwrap();
    EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap()
}

fn reachable<N: WordElement>(analysis: &analysis::EvmAnalysis<N>, pc: usize) -> bool {
    analysis
        .states()
        .iter()
        .any(|state| analysis.program().blocks()[state.key.basic_block_index].start_pc == pc)
}

fn world_reachable(analysis: &EvmWorldAnalysis<NumericValue>, address: Address, pc: usize) -> bool {
    analysis.states().iter().any(|state| {
        state.active().code_address == address
            && state
                .program()
                .and_then(|program| program.blocks().get(state.active().basic_block_index))
                .is_some_and(|block| block.start_pc == pc)
    })
}

fn complete<N: WordElement>(analysis: &analysis::EvmAnalysis<N>) {
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{} work={} {:?}",
        std::any::type_name::<N>(),
        analysis.execution().work(),
        analysis.frontiers()
    );
    assert!(!analysis.states().is_empty());
    assert!(!analysis.execution().outcomes().is_empty());
}

fn address(word: u16) -> Address {
    Address::from_word(U256::from(word).into())
}

fn world(root: &str, callee: &str) -> (EvmWorld<NumericValue>, EvmEntry<NumericValue>) {
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "relational integration fixture");
    for (number, code) in [(0x101, root), (0x200, callee)] {
        let mut account = EvmAccount::<NumericValue>::from_hex(code, Fork::Osaka).unwrap();
        account.balance = EvmValue::<NumericValue>::constant(U256::from(1_000_000));
        world.insert(address(number), account).unwrap();
    }
    (world, EvmEntry::<NumericValue>::new(address(0x101)))
}

#[test]
fn repeated_callvalue_guards_cannot_make_one_input_equal_one_and_two() {
    check_repeated_callvalue_guards_cannot_make_one_input_equal_one_and_two(&native::product());
    check_repeated_callvalue_guards_cannot_make_one_input_equal_one_and_two(&native::finite_with(
        native::spec(8, 4, 256),
    ));
}

fn check_repeated_callvalue_guards_cannot_make_one_input_equal_one_and_two<D: EvmNumericAlgebra>(
    values: &ValueAlgebra<D>,
) {
    let model = std::any::type_name::<D::Element>();

    // First true edge establishes x=1; the later x=2 true edge is impossible.
    let code = "3480600114600957005b80600214601257005b00";

    let result = relations_case(values, code, EvmEnvironmentValues::<D::Element>::default());
    complete(&result);
    assert!(reachable(&result, 9), "x=1 must remain feasible");
    assert!(
        reachable(&result, 17),
        "x=1 must take the second false edge"
    );
    assert!(
        !reachable(&result, 18),
        "x=1 and x=2 cannot share one execution: {model}"
    );
}

#[test]
fn derived_addition_guard_refines_its_input_instead_of_an_independent_top() {
    check_derived_addition_guard_refines_its_input_instead_of_an_independent_top(&native::product());
    check_derived_addition_guard_refines_its_input_instead_of_an_independent_top(
        &native::finite_with(native::spec(8, 4, 256)),
    );
}

fn check_derived_addition_guard_refines_its_input_instead_of_an_independent_top<
    D: EvmNumericAlgebra,
>(
    values: &ValueAlgebra<D>,
) {
    let model = std::any::type_name::<D::Element>();

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

    let result = relations_case(
        values,
        &fixture.code,
        EvmEnvironmentValues::<D::Element>::default(),
    );
    complete(&result);
    assert!(reachable(&result, fixture.labels["derived"]));
    assert!(reachable(&result, fixture.labels["good"]));
    assert!(
        !reachable(&result, fixture.labels["bad"]),
        "x+1=2 implies x=1: {model}"
    );
}

#[test]
fn symbolic_addition_uses_evm_word_wraparound_and_preserves_the_wrapped_path() {
    check_symbolic_addition_uses_evm_word_wraparound_and_preserves_the_wrapped_path(
        &native::product(),
    );
    check_symbolic_addition_uses_evm_word_wraparound_and_preserves_the_wrapped_path(
        &native::finite_with(native::spec(8, 4, 256)),
    );
}

fn check_symbolic_addition_uses_evm_word_wraparound_and_preserves_the_wrapped_path<
    D: EvmNumericAlgebra,
>(
    values: &ValueAlgebra<D>,
) {
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

    let result = relations_case(
        values,
        &fixture.code,
        EvmEnvironmentValues::<D::Element>::default(),
    );
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

#[test]
fn false_edges_exclude_the_same_equality_on_later_blocks() {
    check_false_edges_exclude_the_same_equality_on_later_blocks(&native::product());
    check_false_edges_exclude_the_same_equality_on_later_blocks(&native::finite_with(
        native::spec(8, 4, 256),
    ));
}

fn check_false_edges_exclude_the_same_equality_on_later_blocks<D: EvmNumericAlgebra>(
    values: &ValueAlgebra<D>,
) {
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

    let result = relations_case(
        values,
        &fixture.code,
        EvmEnvironmentValues::<D::Element>::default(),
    );
    complete(&result);
    assert!(reachable(&result, fixture.labels["one"]));
    assert!(reachable(&result, fixture.labels["not_one"]));
    assert!(
        !reachable(&result, fixture.labels["bad"]),
        "false guard means x!=1"
    );
}

#[test]
fn joined_unknown_paths_include_both_values_and_selected_paths_exclude_a_third() {
    check_joined_unknown_paths_include_both_values_and_selected_paths_exclude_a_third(
        &native::product(),
    );
    check_joined_unknown_paths_include_both_values_and_selected_paths_exclude_a_third(
        &native::finite_with(native::spec(8, 4, 256)),
    );
}

fn check_joined_unknown_paths_include_both_values_and_selected_paths_exclude_a_third<
    D: EvmNumericAlgebra,
>(
    values: &ValueAlgebra<D>,
) {
    let model = std::any::type_name::<D::Element>();

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

    let result = relations_case(
        values,
        &fixture.code,
        EvmEnvironmentValues::<D::Element>::default(),
    );
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
            "lost feasible {branch}: {model}"
        );
    }
    // The unknown join selector keeps x=1 OR x=2 in the path DAG. Local
    // projection imports common facts only, so it need not exclude x=3.
    // Reachability is an overapproximation, never a satisfying valuation.
    for (selector, selected, excluded) in [(0, "two", "one"), (1, "one", "two")] {
        let selected_result = relations_case(
            values,
            &fixture.code,
            EvmEnvironmentValues {
                calldata: evm_abstract::world::EvmByteArray::<D::Element>::exact(&[selector]),
                ..EvmEnvironmentValues::<D::Element>::default()
            },
        );
        complete(&selected_result);
        assert!(reachable(&selected_result, fixture.labels[selected]));
        assert!(!reachable(&selected_result, fixture.labels[excluded]));
        assert!(
            !reachable(&selected_result, fixture.labels["bad"]),
            "a known selector must retain its guarded value: {model}"
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
fn loop_fixed_point_forgets_cross_field_equality_without_rebinding_immutable_inputs() {
    let fixture = bounded_loop();
    let environment = EvmEnvironmentValues::<NumericValue>::default();
    let no_loop = relations_case(&native::product(), "34341400", environment.clone());
    complete(&no_loop);
    assert!(no_loop.frontiers().is_empty());
    assert!(no_loop.execution().frontiers().is_empty());
    assert!(!no_loop.states().is_empty());
    for state in no_loop.states() {
        assert_eq!(
            state.exit_stack.last().unwrap().singleton(),
            Some(U256::ONE),
            "CALLVALUE; CALLVALUE; EQ remains exact without a loop"
        );
    }
    // Loop projection forgets the equality between the stack and frame call
    // value. Their independent bounds still contain every original (v, v)
    // pair; the historical immutable input is not rebound to another value.
    for value in [None, Some(U256::ZERO), Some(U256::ONE), Some(U256::MAX)] {
        let mut input = environment.clone();
        if let Some(value) = value {
            input.value = EvmValue::<NumericValue>::constant(value);
        }
        // All cases use the original loop, default policy and actual widening.
        let result = relations_case(&native::product(), &fixture.code, input);
        complete(&result);
        assert!(result.frontiers().is_empty());
        assert!(result.execution().frontiers().is_empty());
        assert!(reachable(&result, fixture.labels["body"]));
        assert!(reachable(&result, fixture.labels["exit"]));
        assert!(
            result.transfers() <= 256,
            "finite counter loop needs widening or a finite fixed point"
        );
        assert!(
            result.states().len() <= 256,
            "loop state count must remain bounded"
        );
        let exits = result
            .states()
            .iter()
            .filter(|state| {
                result.program().blocks()[state.key.basic_block_index].start_pc
                    == fixture.labels["exit"]
            })
            .collect::<Vec<_>>();
        assert!(!exits.is_empty());
        for state in exits {
            let equality = state.exit_stack.last().unwrap();
            assert!(
                equality.contains(U256::ONE),
                "loop abstraction must retain the true equality: CALLVALUE={value:?}"
            );
            assert!(
                equality.interval().unsigned_bounds().1 <= U256::ONE,
                "EQ must retain a Boolean numeric bound"
            );
            if value.is_some() {
                assert_eq!(
                    equality.singleton(),
                    Some(U256::ONE),
                    "concrete CALLVALUE={value:?} stays equal through the loop"
                );
            }
        }
    }
}

#[test]
fn a_loop_cut_short_by_the_shared_transfer_budget_remains_typed_incomplete() {
    let fixture = bounded_loop();
    let result = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex(&fixture.code).unwrap(),
            EvmEnvironmentValues::<NumericValue>::default(),
        )
        .unwrap(),
        ExecutionConfig {
            analysis: Config {
                context_depth: 0,
                max_transfers: 1,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
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
        let result = native::run(
            &native::product(),
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig::default(),
            &Control::default(),
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
    let result = native::run(
        &native::product(),
        AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
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

fn outcome_relations(result: &EvmWorldAnalysis<NumericValue>) -> BTreeSet<String> {
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
    let bounded = EvmValue::<NumericValue>::unsigned_range(U256::from(1), U256::from(3))
        .unwrap()
        .with_expression(a.clone());
    let expanded = EvmValue::<NumericValue>::unsigned_range(U256::from(1), U256::from(4))
        .unwrap()
        .with_expression(c.clone());
    assert_ne!(
        canonical(serde_json::to_value(&bounded).unwrap()),
        canonical(serde_json::to_value(&expanded).unwrap())
    );
    let sourced = EvmValue::<NumericValue>::constant(U256::from(1)).with_expression(a.clone());
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
        let result = native::run(
            &native::product(),
            AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
            ExecutionConfig {
                use_summaries: enabled,
                analysis: Config {
                    context_depth: 0,
                    ..Config::default()
                },
                ..ExecutionConfig::default()
            },
            &Control::default(),
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
    let result = native::run(
        &native::product(),
        AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
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
    let domain = native::product_with(result.domain_spec());
    let original = result.states()[record.source_state]
        .exit
        .as_ref()
        .unwrap()
        .active()
        .memory
        .read_word(
            &EvmValue::<NumericValue>::constant(U256::ZERO),
            domain.borrowed(),
        );
    let original_leaves = original
        .expression()
        .expect("unknown GAS word remains symbolic")
        .fresh_leaves();
    assert!(!original_leaves.is_empty(), "GAS is a fresh runtime value");
    for &replayed in &record.reused_at {
        let state = &result.states()[replayed];
        assert_eq!(state.active().code_address, address(0x200));
        let imported = state.exit.as_ref().unwrap().active().memory.read_word(
            &EvmValue::<NumericValue>::constant(U256::ZERO),
            domain.borrowed(),
        );
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
    let result = relations_case(
        &native::product(),
        &fixture.code,
        EvmEnvironmentValues::<NumericValue>::default(),
    );
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
    let result = relations_case(
        &native::product(),
        &fixture.code,
        EvmEnvironmentValues::<NumericValue>::default(),
    );
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
    let result = relations_case(
        &native::product(),
        &fixture.code,
        EvmEnvironmentValues::<NumericValue>::default(),
    );
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
    let result = native::run(
        &native::product(),
        AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig {
            max_work: 1,
            ..ExecutionConfig::default()
        },
        &Control::default(),
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
) -> analysis::EvmAnalysis<NumericValue> {
    native::run(
        &native::product_with(native::spec(8, 4, 256).with_relations(limits.to_owned())),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex(&fixture.code).unwrap(),
            EvmEnvironmentValues::<NumericValue>::default(),
        )
        .unwrap(),
        ExecutionConfig {
            analysis: Config {
                context_depth: 0,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap()
}

fn retained_unknown_branches(result: &analysis::EvmAnalysis<NumericValue>, fixture: &Fixture) {
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
        relational::{CheckResult, EvmValueQuery, QueryReason, RelationLimits, RelationState},
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
        relations.assume(
            &guard,
            true,
            &policy,
            native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
        ),
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
        relations.local_value(
            &input,
            &bounded,
            native::product_with(native::spec(8, 4, 256).with_relations(bounded)).borrowed()
        ),
        EvmValueQuery::<NumericValue>::Undetermined(QueryReason::ExpressionLimit)
    );
    assert_eq!(relations, before, "interruption must preserve the guard");
    assert_eq!(
        relations.local_value(
            &input,
            &policy,
            native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
        ),
        EvmValueQuery::<NumericValue>::Exact(U256::ONE)
    );
    assert_eq!(
        relations.can_equal(
            &input,
            U256::ZERO,
            &policy,
            native::product_with(native::spec(8, 4, 256).with_relations(policy)).borrowed()
        ),
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
    let mut numeric_limits = native::product().spec().relations();

    use evm_abstract::domain::relational::QueryReason;
    let owner = address(0x101);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "initial state construction limit");
    let mut account = EvmAccount::<NumericValue>::from_hex("00", Fork::Osaka).unwrap();
    account.storage = (0..65)
        .map(|i| {
            (
                U256::from(i),
                EvmValue::<NumericValue>::constant(U256::from(i + 1)),
            )
        })
        .collect();
    world.insert(owner, account).unwrap();
    let entry = EvmEntry::<NumericValue>::new(owner);
    let limited = native::run(
        &native::product_with(native::spec(8, 4, 256).with_relations(numeric_limits)),
        AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(limited.status(), Status::Incomplete);
    assert!(limited.outcomes().is_empty());
    assert!(limited.frontiers().iter().any(|frontier| {
        frontier.reason == FrontierReason::Relations(QueryReason::ExpressionLimit)
    }));
    let config = ExecutionConfig::default();
    numeric_limits.max_depth = 128;
    let domain = native::product_with(native::product().spec().with_relations(numeric_limits));
    let result = native::run(
        &native::product_with(native::spec(8, 4, 256).with_relations(numeric_limits)),
        AnalysisInput::new(world, entry).unwrap(),
        config,
        &Control::default(),
    )
    .unwrap();
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
                .read_checked(
                    owner,
                    &EvmValue::<NumericValue>::constant(U256::from(64)),
                    domain.borrowed()
                )
                .unwrap()
                .singleton(),
            Some(U256::from(65))
        );
    }
}

#[test]
fn cancelled_range_analysis_can_be_retried_without_losing_either_outcome() {
    use evm_abstract::analysis::{control::Control, progress::Observer};
    let (world, mut entry) = world("5f345f3e00", "00");
    entry.environment.value = EvmValue::<NumericValue>::top();
    // Replace the removed fuel knob with an actual cancellation boundary. Both
    // the retry and concrete controls keep the original execution defaults.
    let control = Control::new(Observer::default());
    control.cancellation().cancel();
    assert!(matches!(
        native::run(
            &native::product(),
            AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
            ExecutionConfig::default(),
            &control
        ),
        Err(evm_abstract::analysis::cooperation::CooperationError::Cancelled)
    ));
    let result = native::run(
        &native::product(),
        AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
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
        concrete.environment.value = EvmValue::<NumericValue>::constant(U256::from(size));
        let control = native::run(
            &native::product(),
            AnalysisInput::new(world.clone(), concrete).unwrap(),
            ExecutionConfig::default(),
            &Control::default(),
        )
        .unwrap();
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
    let mut numeric_limits = native::product().spec().relations();

    use evm_abstract::domain::relational::{EvmValueQuery, QueryReason};
    let (world, entry) = world("5f3500", "00");
    let config = ExecutionConfig::default();
    // This replaces only the obsolete native-fuel fixture. Sixty-four nodes
    // retain the read graph, but cannot pay the real 32-byte local projection.
    numeric_limits.max_nodes = 64;
    let analysis = native::run(
        &native::product_with(native::spec(8, 4, 256).with_relations(numeric_limits)),
        AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
        config.clone(),
        &Control::default(),
    )
    .unwrap();
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
    assert!(expression.nodes() <= numeric_limits.max_nodes);
    assert!(expression.depth() <= numeric_limits.max_depth);
    assert_eq!(
        evm_abstract::domain::relational::RelationState::default().local_value(
            expression,
            &numeric_limits,
            native::product_with(native::spec(8, 4, 256).with_relations(numeric_limits)).borrowed()
        ),
        EvmValueQuery::<NumericValue>::Undetermined(QueryReason::ExpressionLimit)
    );
    let EvmValueQuery::<NumericValue>::Bounds(bounds) =
        evm_abstract::domain::relational::RelationState::default().local_value(
            expression,
            &Default::default(),
            native::product_with(native::spec(8, 4, 256).with_relations(Default::default()))
                .borrowed(),
        )
    else {
        panic!("the default local policy must complete the same read");
    };
    for value in [U256::ZERO, U256::MAX] {
        assert!(bounds.contains(value), "local read bounds excluded {value}");
    }
    let mut concrete = entry;
    concrete.environment.calldata =
        evm_abstract::world::EvmByteArray::<NumericValue>::exact(&[7; 32]);
    let exact = native::run(
        &native::product_with(native::spec(8, 4, 256).with_relations(numeric_limits)),
        AnalysisInput::new(world, concrete).unwrap(),
        config,
        &Control::default(),
    )
    .unwrap();
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
    use evm_abstract::{
        analysis::{FrameCode, OutcomeKind},
        domain::{
            relational::{QueryReason, RelationLimits},
            symbolic::{ExprId, SlotStateKind},
        },
        world::{EvmStore, Symbol},
    };
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
        let config = Config {
            context_depth: 0,
            ..Config::default()
        };
        let symbolic_environment = EvmEnvironmentValues {
            calldata: evm_abstract::world::EvmByteArray::<NumericValue>::empty(),
            ..EvmEnvironmentValues::<NumericValue>::default()
        };
        let environment = EvmEnvironmentValues {
            // Keep mandatory recipient cleanup out of the optional scalar
            // control; CALLVALUE stays unknown and the bytecode stays identical.
            to: address(0x101).into(),
            ..symbolic_environment.clone()
        };
        let result = native::run(
            &native::product_with(native::spec(8, 4, 256).with_relations(limits)),
            AnalysisInput::<NumericValue>::from_program(
                Program::from_hex(&fixture.code).unwrap(),
                environment.clone(),
            )
            .unwrap(),
            ExecutionConfig {
                analysis: config.clone(),
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
        .unwrap();
        complete(&result);
        assert!(result.frontiers().is_empty());
        assert!(result.execution().frontiers().is_empty());
        for branch in ["true", "false"] {
            assert!(reachable(&result, fixture.labels[branch]));
        }
        // The numeric result survives; forgetting its optional expression
        // loses correlation without leaving either branch unexpanded.
        assert!(
            result.diagnostics().iter().any(|diagnostic| {
                matches!(
                    diagnostic.kind,
                    analysis::DiagnosticKind::RelationalPrecisionLost(QueryReason::ExpressionLimit)
                )
            }),
            "optional expression exhaustion must report precision loss"
        );
        let execution = result.execution();
        let source = &execution.states()[0];
        let owner = address(0x101);
        assert_eq!(result.environment(), &environment);
        assert_eq!(execution.entry().address, owner);
        assert_eq!(source.id, 0);
        assert_eq!(source.key.frames.len(), 1);
        assert_eq!(source.key, source.entry.key());
        assert_eq!(source.program(), Some(result.program()));
        assert_eq!(source.active().code_address, owner);
        assert_eq!(source.active().address, owner);
        assert_eq!(source.active().address_value, environment.to);
        assert_eq!(source.active().basic_block_index, 0);
        assert!(source.active().jump_history.is_empty());
        let input_store = EvmStore::<NumericValue>::new_checked(
            execution.world(),
            native::product_with(execution.domain_spec()).borrowed(),
        )
        .unwrap();
        let input_root = input_store.slot_root(owner, false).unwrap();
        assert!(matches!(
            input_root.kind(),
            SlotStateKind::Input { bindings, .. } if bindings.is_empty()
        ));
        for value in [U256::ZERO, U256::ONE] {
            assert!(input_store.storage_default(owner).contains(value));
        }
        let mut entered_store = input_store.clone();
        entered_store.touch_account(owner);
        assert_eq!(source.entry.store, entered_store);
        assert_eq!(source.entry.active().saved_store.state(), &input_store);
        let call_value = &source.entry.active().call_value;
        let input = call_value.identity().input().expect("the CALLVALUE input");
        assert_eq!(input.symbol(), Symbol::CallValue);
        assert_eq!(call_value.numeric(), environment.value.numeric());
        assert_eq!(
            call_value.expression(),
            Some(&ExprId::input(input.scope(), Symbol::CallValue))
        );
        assert_eq!(
            source.exit.as_ref().unwrap().active().call_value,
            *call_value
        );
        let comparison_index = result.program().blocks()[0]
            .instructions
            .iter()
            .position(|instruction| instruction.opcode == opcode::EQ)
            .unwrap();
        let comparison = source
            .execution_evidence()
            .unwrap()
            .instruction_values()
            .unwrap()
            .iter()
            .find(|value| value.instruction_index() == comparison_index)
            .expect("the original source executed the comparison");
        assert_eq!(
            source.executed_pcs[comparison_index],
            result.program().blocks()[0].instructions[comparison_index].pc
        );
        for (value, branch) in [(U256::ONE, "true"), (U256::ZERO, "false")] {
            assert!(comparison.numeric().contains(value));
            let branch = execution
                .edges()
                .iter()
                .filter(|edge| edge.from == source.id)
                .map(|edge| &execution.states()[edge.to])
                .find(|state| {
                    result.program().blocks()[state.active().basic_block_index].start_pc
                        == fixture.labels[branch]
                })
                .expect("the same source reaches this feasible branch");
            let mut expected_key = source.key.clone();
            expected_key.frames[0].basic_block_index = branch.active().basic_block_index;
            assert_eq!(branch.key, expected_key);
            assert_eq!(branch.entry.key(), expected_key);
            assert_eq!(branch.entry.active().call_value, *call_value);
            assert_eq!(branch.entry.active().saved_store.state(), &input_store);
            assert_eq!(branch.entry.store, entered_store);
            let returned = execution
                .outcomes()
                .iter()
                .find(|outcome| outcome.state == branch.id && outcome.kind == OutcomeKind::Return)
                .expect("this branch completes successfully");
            // Compare the current numeric facts together with their original
            // immutable source, rather than accepting an unrelated top value.
            for store in [&branch.entry.store, &returned.store] {
                assert_eq!(
                    (
                        store.slot_root(owner, false).unwrap(),
                        store.storage_default(owner),
                    ),
                    (input_root.clone(), input_store.storage_default(owner)),
                );
            }
        }
        for (value, selected, excluded) in [(1, "true", "false"), (0, "false", "true")] {
            let witness = native::run(
                &native::product_with(native::spec(8, 4, 256).with_relations(limits)),
                AnalysisInput::<NumericValue>::from_program(
                    Program::from_hex(&fixture.code).unwrap(),
                    EvmEnvironmentValues {
                        value: EvmValue::<NumericValue>::constant(U256::from(value)),
                        ..environment.clone()
                    },
                )
                .unwrap(),
                ExecutionConfig {
                    analysis: config.clone(),
                    ..ExecutionConfig::default()
                },
                &Control::default(),
            )
            .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
            .unwrap();
            complete(&witness);
            assert!(witness.frontiers().is_empty());
            assert!(witness.execution().frontiers().is_empty());
            assert!(reachable(&witness, fixture.labels[selected]));
            assert!(
                !reachable(&witness, fixture.labels[excluded]),
                "CALLVALUE={value} witnesses only the {selected} branch"
            );
        }

        // The original symbolic recipient needs a mandatory lifecycle root
        // before the first opcode, independently of optional scalar precision.
        let boundary = native::run(
            &native::product_with(native::spec(8, 4, 256).with_relations(limits)),
            AnalysisInput::<NumericValue>::from_program(
                Program::from_hex(&fixture.code).unwrap(),
                symbolic_environment.clone(),
            )
            .unwrap(),
            ExecutionConfig {
                analysis: config.clone(),
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
        .unwrap();
        assert_eq!(boundary.status(), Status::Incomplete);
        assert_eq!(boundary.environment(), &symbolic_environment);
        let execution = boundary.execution();
        assert_eq!(execution.status(), Status::Incomplete);
        let [frontier] = execution.frontiers() else {
            panic!("exactly one mandatory retention boundary is required");
        };
        assert_eq!(
            frontier.reason,
            FrontierReason::Relations(QueryReason::ExpressionLimit)
        );
        assert_eq!(frontier.pc, Some(0));
        assert_eq!(frontier.from, Some(0));
        assert!(frontier.range.is_none());
        assert!(frontier.payload.is_none());
        let source = &execution.states()[frontier.from.unwrap()];
        assert_eq!(execution.states().len(), 1);
        assert_eq!(source.id, 0);
        assert_eq!(frontier.target.as_ref(), Some(&source.key));
        assert_eq!(source.key, source.entry.key());
        assert_eq!(source.key.frames.len(), 1);
        let owner = execution.entry().address;
        assert_eq!(owner, Address::repeat_byte(0x11));
        let frame = source.entry.active();
        assert_eq!(frame.key.address, owner);
        assert_eq!(frame.key.code_address, owner);
        assert_eq!(frame.key.address_value, AddressInput::unknown_to());
        assert_eq!(frame.key.caller, symbolic_environment.caller);
        assert_eq!(frame.key.is_static, symbolic_environment.is_static);
        assert_eq!(frame.key.mode, FrameCode::Runtime);
        assert_eq!(frame.key.basic_block_index, 0);
        assert_eq!(frame.key.stack_height, 0);
        assert!(frame.key.jump_history.is_empty());
        assert_eq!(frame.program.as_ref(), Some(boundary.program()));
        assert_eq!(frame.calldata, symbolic_environment.calldata);
        assert!(frame.stack.is_empty());
        assert!(source.executed_pcs.is_empty());
        assert!(source.exit_stack.is_empty());
        assert_eq!(source.exit.as_ref(), Some(&source.entry));
        assert!(execution.edges().is_empty());
        let [projected] = boundary.frontiers() else {
            panic!("the learning view must retain the same boundary");
        };
        assert_eq!(projected.reason, frontier.reason);
        assert_eq!(projected.limit, Limit::Model);
        assert_eq!(projected.from, Some(0));
        assert_eq!(boundary.execution_state_id(0), Some(source.id));
        assert_eq!(projected.target, boundary.states()[0].key);

        let input_store = EvmStore::<NumericValue>::new_checked(
            execution.world(),
            native::product_with(execution.domain_spec()).borrowed(),
        )
        .unwrap();
        let mut entered_store = input_store.clone();
        entered_store.may_touch_all();
        assert_eq!(source.entry.store, entered_store);
        assert_eq!(frame.saved_store.state(), &input_store);
        assert_eq!(source.entry.store.touched_default(), None);
        assert_eq!(frame.saved_store.state().touched_default(), Some(false));
        assert_eq!(&frame.call_value, call_value);
        let input = frame
            .call_value
            .identity()
            .input()
            .expect("retained CALLVALUE");
        assert_eq!(input.symbol(), Symbol::CallValue);
        assert_eq!(
            frame.call_value.numeric(),
            symbolic_environment.value.numeric()
        );
        assert_eq!(
            frame.call_value.expression(),
            Some(&ExprId::input(input.scope(), Symbol::CallValue))
        );
        for value in [U256::ZERO, U256::ONE] {
            assert!(frame.call_value.contains(value));
        }
        let ripemd = address(3);
        assert_ne!(
            input_store.slot_root(owner, false).unwrap(),
            input_store.slot_root(ripemd, false).unwrap()
        );
        for owner in [owner, ripemd] {
            let input_root = input_store.slot_root(owner, false).unwrap();
            assert!(matches!(
                input_root.kind(),
                SlotStateKind::Input { source, bindings }
                    if bindings.is_empty()
                        && source.namespace() == U256::from_be_slice(owner.as_slice()) << 8
            ));
            for value in [U256::ZERO, U256::ONE] {
                assert!(input_store.storage_default(owner).contains(value));
            }
            assert_eq!(
                (
                    source.entry.store.slot_root(owner, false).unwrap(),
                    source.entry.store.storage_default(owner),
                ),
                (input_root, input_store.storage_default(owner)),
            );
            assert_eq!(source.entry.store.touched_in_transaction(owner), None);
            assert_eq!(
                frame.saved_store.state().touched_in_transaction(owner),
                Some(false)
            );
        }
        // Initialization already records pre-entry gas failure. It keeps the
        // untouched checkpoint and must not be mistaken for opcode execution.
        let [failure] = execution.outcomes() else {
            panic!("only the pre-entry failure survives mandatory retention refusal");
        };
        assert_eq!(failure.state, source.id);
        assert_eq!(failure.kind, OutcomeKind::Failure);
        assert_eq!(&failure.store, frame.saved_store.state());
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
        analysis::DiagnosticKind, domain::relational::EvmValueQuery, world::EvmByteArray,
    };
    // A partial overwrite of a 128-byte symbolic copy retains a content term.
    // Its ordinary unknown value is not an unsupported-provider diagnostic.
    let result = native::run(
        &native::product(),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex("60805f5f3760ff5f5360805f205000").unwrap(),
            EvmEnvironmentValues::<NumericValue>::default(),
        )
        .unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
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
    let domain = native::product_with(result.execution().domain_spec());
    let materialize_limit = ExecutionConfig::default().max_memory_bytes;
    let input = payload
        .active()
        .memory
        .slice(
            &EvmValue::<NumericValue>::constant(U256::ZERO),
            &EvmValue::<NumericValue>::constant(U256::from(128)),
            domain.borrowed(),
        )
        .unwrap();
    let hash = input.keccak(domain.borrowed(), materialize_limit).unwrap();
    let answer = payload.relations.local_value(
        hash.expression().unwrap(),
        &domain.spec().relations(),
        native::product_with(native::spec(8, 4, 256).with_relations(domain.spec().relations()))
            .borrowed(),
    );
    let EvmValueQuery::<NumericValue>::Bounds(bounds) = answer else {
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
        let exact = EvmByteArray::<NumericValue>::exact(&bytes)
            .keccak(domain.borrowed(), materialize_limit)
            .unwrap();
        assert_eq!(exact.singleton(), Some(expected));
        assert_eq!(
            evm_abstract::domain::relational::RelationState::default().local_value(
                exact.expression().unwrap(),
                &domain.spec().relations(),
                native::product_with(
                    native::spec(8, 4, 256).with_relations(domain.spec().relations())
                )
                .borrowed()
            ),
            EvmValueQuery::<NumericValue>::Exact(expected)
        );
    }
}

#[test]
fn disabling_relations_restores_the_broader_baseline_cfg() {
    use evm_abstract::domain::relational::RelationLimits;
    let code = "3480600114600957005b80600214601257005b00";
    let result = native::run(
        &native::product_with(native::spec(8, 4, 256).with_relations(RelationLimits {
            enabled: false,
            ..RelationLimits::default()
        })),
        AnalysisInput::<NumericValue>::from_program(
            Program::from_hex(code).unwrap(),
            EvmEnvironmentValues::<NumericValue>::default(),
        )
        .unwrap(),
        ExecutionConfig {
            analysis: Config {
                context_depth: 0,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .map(|graph| EvmAnalysis::from_execution(graph, &mut native::postprocessing()).unwrap())
    .unwrap();
    complete(&result);
    assert!(
        reachable(&result, 18),
        "the ablation must not quietly retain relational guard pruning"
    );
}
