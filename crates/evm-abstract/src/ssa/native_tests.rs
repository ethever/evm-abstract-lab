//! One native control-flow path for independent product, finite and custom carriers.

use super::{FactScope, build, build_normalized_world, build_partial_world, build_world};
use crate::{
    Address, Fork, U256,
    analysis::{Config, EvmAnalysis, ExecutionConfig, MachineEdgeKind, Status},
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, FiniteConstantSet, FiniteSetAlgebra,
        NumericReduction, ProductAlgebra, ValueAlgebra, WordElement, WordObservation,
    },
    ssa::test_support::{postprocessing, run_native},
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use serde::Serialize;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CustomWord {
    Open,
    Exact { word: U256 },
    ExceptTwo,
}
impl fmt::Display for CustomWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => f.write_str("custom(any)"),
            Self::ExceptTwo => f.write_str("custom(word != 2)"),
            Self::Exact { word } => write!(f, "custom({word:#x})"),
        }
    }
}
impl WordElement for CustomWord {
    fn top() -> Self {
        Self::Open
    }
    fn constant(word: U256) -> Self {
        Self::Exact { word }
    }
    fn from_observation(value: &WordObservation) -> Self {
        value.singleton().map_or(Self::Open, Self::constant)
    }
    fn observation(&self) -> WordObservation {
        match self {
            Self::Exact { word } => WordObservation::constant(*word),
            _ => WordObservation::top(),
        }
    }
    fn singleton(&self) -> Option<U256> {
        match self {
            Self::Exact { word } => Some(*word),
            _ => None,
        }
    }
    fn contains(&self, word: U256) -> bool {
        match self {
            Self::Exact { word: exact } => word == *exact,
            Self::ExceptTwo => word != U256::from(2),
            Self::Open => true,
        }
    }
    fn is_top(&self) -> bool {
        matches!(self, Self::Open)
    }
    fn work_size(&self) -> usize {
        8
    }
    fn clone_work(&self) -> usize {
        8
    }
    fn singleton_work(&self) -> usize {
        8
    }
}
#[derive(Debug)]
struct CustomAlgebra(DomainSpec);
impl EvmNumericAlgebra for CustomAlgebra {
    type Element = CustomWord;
    fn spec(&self) -> DomainSpec {
        self.0
    }
    fn project_numeric(&self, value: &CustomWord) -> CustomWord {
        value.clone()
    }
    fn join_numeric(&self, a: &CustomWord, b: &CustomWord) -> CustomWord {
        if a == b {
            a.clone()
        } else if !a.contains(U256::from(2)) && !b.contains(U256::from(2)) {
            CustomWord::ExceptTwo
        } else {
            CustomWord::Open
        }
    }
    fn widen_numeric(&self, a: &CustomWord, b: &CustomWord) -> CustomWord {
        self.join_numeric(a, b)
    }
    fn widen_procedure_numeric(&self, a: &CustomWord, b: &CustomWord) -> CustomWord {
        self.join_numeric(a, b)
    }
    fn reduce_numeric(&self, value: &CustomWord) -> NumericReduction<CustomWord> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a CustomWord>,
    ) -> NumericReduction<CustomWord>
    where
        CustomWord: 'a,
    {
        let operands = args.map(WordElement::singleton).collect::<Option<Vec<_>>>();
        let result = operands
            .filter(|args| crate::domain::concrete::arity(op) == Some(args.len()))
            .map_or(CustomWord::Open, |args| {
                CustomWord::constant(crate::domain::concrete::evaluate(
                    op,
                    args[0],
                    args.get(1).copied().unwrap_or(U256::ZERO),
                    args.get(2).copied().unwrap_or(U256::ZERO),
                ))
            });
        NumericReduction::stable(result)
    }
    fn operation_work<'a>(&self, _op: u8, args: impl Iterator<Item = &'a CustomWord>) -> usize
    where
        CustomWord: 'a,
    {
        (if _op == revm_bytecode::opcode::EXP {
            64_000
        } else {
            4096
        }) + args.count() * 32
    }
    fn literal_quote_work(&self, _: U256) -> usize {
        32
    }
    fn projection_work(&self, _: &CustomWord) -> usize {
        32
    }
    fn refinement_work(&self, _: &CustomWord, facts: usize) -> usize {
        4096 + facts * 32
    }
}
fn spec() -> DomainSpec {
    DomainSpec::default().with_relations(crate::domain::relational::RelationLimits {
        enabled: false,
        ..Default::default()
    })
}
fn control_flow<D: EvmNumericAlgebra>(
    algebra: &ValueAlgebra<D>,
    value: D::Element,
) -> super::NormalizedSsa<D::Element> {
    let mut postprocess = postprocessing();
    let root = Address::repeat_byte(0x11);
    let mut world = EvmWorld::new(Fork::Osaka, "generic SSA diamond");
    world
        .insert(
            root,
            EvmAccount::from_hex("3480600a576002600d565b60015b600a0100", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(
        root,
        root,
        EvmValue::from_numeric(value),
        EvmByteArray::empty(),
    );
    let execution = run_native(
        algebra,
        world,
        entry,
        ExecutionConfig {
            analysis: Config {
                context_depth: 0,
                ..Config::default()
            },
            use_summaries: false,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(
        execution.status(),
        Status::Converged,
        "{:?}",
        execution.frontiers()
    );
    assert!(execution.edges().len() >= 4);
    let raw = build_world(&execution).unwrap();
    let normalized = build_normalized_world(&execution, &mut postprocess).unwrap();
    normalized.verify(&execution, &mut postprocess).unwrap();
    assert_eq!(raw.value_count(), normalized.value_count());
    assert_eq!(raw.effect_count(), normalized.effect_count());
    let value = &execution.entry().environment.value;
    let json = crate::render::world::json(&execution).unwrap();
    assert_eq!(json["schema_version"], 12);
    assert_eq!(
        json["entry"]["environment"]["value"]["numeric"],
        serde_json::to_value(value.numeric()).unwrap()
    );
    let local = EvmAnalysis::from_execution(execution, &mut postprocess).unwrap();
    build(&local).unwrap().verify(&local).unwrap();
    for state in local.states() {
        assert!(local.execution_state_id(state.id).is_some());
    }
    assert!(crate::render::cfg(&local, algebra).contains("stack in"));
    assert!(crate::render::world::normalized_ssa(local.execution(), &normalized).contains("ADD"));
    normalized
}
#[test]
fn product_and_finite_carriers_keep_the_same_verified_control_flow() {
    control_flow(
        &ValueAlgebra::new(ProductAlgebra::from_spec(spec())),
        <crate::domain::NumericValue as WordElement>::top(),
    );
    let finite = control_flow(
        &ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec())),
        FiniteConstantSet::top(),
    );
    let facts = serde_json::to_value(finite.facts()).unwrap();
    assert!(
        facts
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|fact| fact.get("numeric"))
            .any(
                |numeric| numeric.get("constants").is_some() && numeric.get("known_bits").is_none()
            )
    );
}
#[test]
fn hidden_custom_predicate_survives_native_facts_and_never_becomes_a_builtin_value() {
    let ir = control_flow(
        &ValueAlgebra::new(CustomAlgebra(spec())),
        CustomWord::ExceptTwo,
    );
    let facts = ir
        .facts()
        .iter()
        .filter(|fact| fact.numeric == Some(CustomWord::ExceptTwo))
        .collect::<Vec<_>>();
    assert!(!facts.is_empty());
    assert!(facts.iter().all(|fact| fact.constant.is_none()));
    assert!(facts.iter().any(|fact| fact.scope == FactScope::Definition));
    let payload = serde_json::to_value(ir.facts()).unwrap();
    assert!(
        payload
            .as_array()
            .unwrap()
            .iter()
            .any(|fact| fact["numeric"]["kind"] == "except_two")
    );
}
fn call_return<D: EvmNumericAlgebra>(algebra: &ValueAlgebra<D>) {
    let mut postprocess = postprocessing();
    let root = Address::repeat_byte(0x11);
    let child = Address::repeat_byte(0x22);
    let mut world = EvmWorld::new(Fork::Osaka, "generic suspended frames");
    world
        .insert(
            root,
            EvmAccount::from_hex(
                &format!("600a5f5f5f5f5f73{child:x}61fffff1505000"),
                Fork::Osaka,
            )
            .unwrap(),
        )
        .unwrap();
    world
        .insert(child, EvmAccount::from_hex("600100", Fork::Osaka).unwrap())
        .unwrap();
    let entry = EvmEntry::concrete(
        root,
        root,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let partial = run_native(
        algebra,
        world.clone(),
        entry.clone(),
        ExecutionConfig {
            analysis: Config {
                max_transfers: 1,
                ..Config::default()
            },
            use_summaries: false,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(partial.status(), Status::Incomplete);
    assert!(!partial.frontiers().is_empty());
    let partial_ir = build_partial_world(&partial, &mut postprocess).unwrap();
    assert_eq!(partial_ir.frontiers().len(), partial.frontiers().len());
    assert_eq!(partial_ir.blocks().len(), partial.states().len());
    let execution = run_native(
        algebra,
        world,
        entry,
        ExecutionConfig {
            use_summaries: false,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(
        execution.status(),
        Status::Converged,
        "{:?}",
        execution.frontiers()
    );
    let ir = build_world(&execution).unwrap();
    assert!(
        ir.transitions()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Call && edge.result.is_none())
    );
    assert!(
        ir.transitions()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Return && edge.result.is_some())
    );
    assert!(
        execution
            .states()
            .iter()
            .any(|state| state.entry.call_stack.depth() == 2)
    );
    let local = EvmAnalysis::from_execution(execution, &mut postprocess).unwrap();
    assert!(local.execution().states().len() > local.states().len());
    let local_ir = build(&local).unwrap();
    local_ir.verify(&local).unwrap();
    assert_eq!(
        local_ir.execution().blocks().len(),
        local.execution().states().len()
    );
    assert!(
        local_ir
            .blocks()
            .iter()
            .flat_map(|block| &block.phis)
            .flat_map(|phi| &phi.inputs)
            .any(|input| local.execution().states()[input.predecessor]
                .entry
                .call_stack
                .depth()
                == 2)
    );
    let rendered = crate::render::ssa(&local, &local_ir, algebra);
    assert!(
        rendered.contains("Call") && rendered.contains("Return"),
        "{rendered}"
    );
    let normalized = super::build_normalized(&local, &mut postprocess).unwrap();
    assert_eq!(normalized.blocks().len(), local.execution().states().len());
}
#[test]
fn all_native_carriers_share_suspended_call_and_return_ssa() {
    call_return(&ValueAlgebra::new(ProductAlgebra::from_spec(spec())));
    call_return(&ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec())));
    call_return(&ValueAlgebra::new(CustomAlgebra(spec())));
}

fn local_rejected_call<D: EvmNumericAlgebra>(algebra: &ValueAlgebra<D>) {
    let mut postprocess = postprocessing();
    let root = Address::repeat_byte(0x11);
    let child = Address::repeat_byte(0x22);
    let mut world = EvmWorld::new(Fork::Osaka, "native rejected call continuation");
    let mut account =
        EvmAccount::from_hex(&format!("5f5f5f5f600173{child:x}61fffff15000"), Fork::Osaka).unwrap();
    account.balance = EvmValue::constant(U256::ZERO);
    world.insert(root, account).unwrap();
    world.insert(child, EvmAccount::empty()).unwrap();
    let entry = EvmEntry::concrete(
        root,
        root,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let execution = run_native(
        algebra,
        world,
        entry,
        ExecutionConfig {
            use_summaries: false,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(
        execution.status(),
        Status::Converged,
        "{:?}",
        execution.frontiers()
    );
    assert!(
        execution
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Failure)
    );
    let local = EvmAnalysis::from_execution(execution, &mut postprocess).unwrap();
    assert!(
        local
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Failure)
    );
    for edge in local.execution().edges() {
        let from = local
            .states()
            .iter()
            .find(|state| local.execution_state_id(state.id) == Some(edge.from));
        let to = local
            .states()
            .iter()
            .find(|state| local.execution_state_id(state.id) == Some(edge.to));
        if let (Some(from), Some(to)) = (from, to) {
            assert!(
                local
                    .edges()
                    .iter()
                    .any(|projected| projected.from == from.id
                        && projected.to == to.id
                        && projected.kind == edge.kind)
            );
        }
    }
    let ir = build(&local).unwrap();
    ir.verify(&local).unwrap();
    assert!(
        ir.blocks()
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|row| row.opcode == revm_bytecode::opcode::CALL)
            .all(|row| row.results.is_empty())
    );
    assert!(
        ir.execution()
            .transitions()
            .iter()
            .any(|transition| transition.kind == MachineEdgeKind::Failure
                && transition.result.is_some())
    );
    assert!(crate::render::ssa(&local, &ir, algebra).contains("Failure"));
    assert!(crate::render::cfg(&local, algebra).contains("Failure"));
    assert!(crate::render::dot(&local, algebra).contains("Failure"));
}
#[test]
fn root_local_rejected_calls_keep_typed_edges_and_deferred_native_results() {
    local_rejected_call(&ValueAlgebra::new(ProductAlgebra::from_spec(spec())));
    local_rejected_call(&ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec())));
    local_rejected_call(&ValueAlgebra::new(CustomAlgebra(spec())));
}
