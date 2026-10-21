use super::{
    carrier::ExcludingWord,
    fixtures::*,
    policy::{Calls, Policy},
};
use alloy_primitives::U256;
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput};
use evm_abstract::{
    analysis::{
        Arrival, ContextPolicy, EnginePolicies, ExecutionConfig, OutcomeKind, PartitionPolicy,
        ProcedureSummaryKind, Status,
    },
    domain::ValueAlgebra,
    render::world::evidence,
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn config() -> ExecutionConfig {
    super::fixtures::config(1_000_000_000)
}
fn policies() -> EnginePolicies {
    EnginePolicies::default()
}

#[test]
fn semantic_exclusion_survives_delegate_checkpoint_and_controls_the_actual_cfg() {
    let fixture = rollback();
    let world = rollback_world(&fixture);
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let result = {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&algebra, &policies(), &mut extension).run(
            AnalysisInput::new(world.clone(), entry()).unwrap(),
            config(),
            &Control::default(),
        )
    }
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(result.frontiers().is_empty());
    assert!(
        !result
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&fixture.impossible))
    );
    let returns: Vec<_> = result
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect();
    assert!(!returns.is_empty());
    for outcome in returns {
        assert_eq!(
            outcome
                .data
                .read_word(&word(0), algebra.borrowed())
                .singleton(),
            Some(U256::ONE)
        );
        for slot in [0, 1] {
            assert!(
                !outcome
                    .store
                    .read(address(OWNER), &word(slot), algebra.borrowed())
                    .contains(U256::from(42))
            );
        }
        assert!(
            outcome.store.possible_logs().is_empty(),
            "delegated child log must roll back"
        );
    }
    assert!(
        result.states().iter().any(
            |state| state.entry.call_stack.children().iter().any(|child| !child
                .state
                .saved_store
                .state()
                .read_transient(address(OWNER), &word(0), algebra.borrowed())
                .contains(U256::from(42)))
        ),
        "a real delegated child must retain the parent checkpoint's transient custom value"
    );
    assert!(calls.arithmetic.load(Ordering::Relaxed) > 0);
    let json = serde_json::to_value(evidence(&result).unwrap()).unwrap();
    assert_eq!(json["status"], "Converged");
    let expected = serde_json::to_value(ExcludingWord::except(U256::from(42))).unwrap();
    fn contains_numeric(json: &serde_json::Value, expected: &serde_json::Value) -> bool {
        match json {
            serde_json::Value::Object(fields) => {
                fields.get("numeric") == Some(expected)
                    || fields
                        .values()
                        .any(|value| contains_numeric(value, expected))
            }
            serde_json::Value::Array(values) => {
                values.iter().any(|value| contains_numeric(value, expected))
            }
            _ => false,
        }
    }
    assert!(
        contains_numeric(&json, &expected),
        "native numeric payload must retain the actual excluded42 predicate"
    );

    // Same DomainSpec and D type, different immutable semantics. Reusing the
    // previous run's narrow branch answer/certificate would be unsound.
    let coarse_calls = Calls::default();
    let mut coarse = Policy::new(&coarse_calls);
    coarse.exclusion_queries = false;
    coarse.retain_exclusions = false;
    let coarse = ValueAlgebra::new(coarse);
    assert_eq!(algebra.spec(), coarse.spec());
    let control = {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&coarse, &policies(), &mut extension).run(
            AnalysisInput::new(world, entry()).unwrap(),
            config(),
            &Control::default(),
        )
    }
    .unwrap();
    assert_eq!(
        control.status(),
        Status::Converged,
        "{:?}",
        control.frontiers()
    );
    assert!(
        control
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&fixture.impossible)),
        "removing the custom exclusion consequence must lose real CFG precision"
    );
}

#[test]
fn singleton_without_constants_resolves_self_call_and_preserves_custom_recursive_outputs() {
    let code = alloy_primitives::hex::decode("5f5f5f5f5f305af150602a5f5260205ff3").unwrap();
    let world = world(&code);
    assert!(word(OWNER).constants().is_none());
    assert_eq!(word(OWNER).singleton(), Some(U256::from(OWNER)));
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let result = {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&algebra, &policies(), &mut extension).run(
            AnalysisInput::new(world, entry()).unwrap(),
            config(),
            &Control::default(),
        )
    }
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(
        !result.procedure_summaries().is_empty(),
        "the test must enter the actual recursive procedure path"
    );
    assert!(
        result
            .procedure_summaries()
            .iter()
            .all(|record| record.kind == ProcedureSummaryKind::KnownRecursion),
        "an exact native singleton target must not acquire an unknown-target remainder"
    );
    assert!(
        result
            .procedure_summaries()
            .iter()
            .all(|record| record.closed)
    );
    for state in result.states() {
        let value = state
            .entry
            .store
            .read(address(OWNER), &word(0), algebra.borrowed());
        eprintln!(
            "recursive carrier state code={:?} pcs={:?}: {}",
            state.active().code_address,
            state.executed_pcs,
            value.numeric()
        );
    }
    for record in result.procedure_summaries() {
        for procedure in &record.procedures {
            assert_eq!(procedure.frame.address, address(OWNER));
            assert_eq!(procedure.frame.code_address, address(OWNER));
            for output in &procedure.outputs {
                eprintln!(
                    "recursive carrier summary {:?}: {}",
                    output.kind,
                    output
                        .store
                        .read(address(OWNER), &word(0), algebra.borrowed())
                        .numeric()
                );
                assert!(
                    !output
                        .store
                        .read(address(OWNER), &word(0), algebra.borrowed())
                        .contains(U256::from(42))
                );
            }
        }
    }
    assert!(
        result
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return)
    );
    for outcome in result
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
    {
        eprintln!(
            "recursive carrier terminal state={} kind={:?}: {}",
            outcome.state,
            outcome.kind,
            outcome
                .store
                .read(address(OWNER), &word(0), algebra.borrowed())
                .numeric()
        );
        assert_eq!(
            outcome
                .data
                .read_word(&word(0), algebra.borrowed())
                .singleton(),
            Some(U256::from(42))
        );
        assert!(
            !outcome
                .store
                .read(address(OWNER), &word(0), algebra.borrowed())
                .contains(U256::from(42))
        );
    }
    assert!(calls.joins.load(Ordering::Relaxed) > 0);
    assert!(calls.projections.load(Ordering::Relaxed) > 0);
}

struct OtherContext<'a> {
    visits: &'a AtomicUsize,
    label: String,
}
impl ContextPolicy for OtherContext<'_> {
    fn advance(&self, history: &mut Vec<usize>, _: usize, _: usize) {
        self.visits.fetch_add(1, Ordering::Relaxed);
        history.clear();
    }
    fn normalize(&self, history: &mut Vec<usize>) {
        history.clear();
    }
    fn work(&self, history: usize) -> usize {
        history.saturating_add(self.label.len())
    }
}
struct OtherPartition<'a>(&'a AtomicUsize);
impl PartitionPolicy for OtherPartition<'_> {
    type Class = ();
    fn classification_work(&self, _: Arrival) -> usize {
        1
    }

    fn classify(&self, _: Arrival) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn alternate_context_partition_and_borrowed_noncopy_policy_reach_the_same_engine() {
    let context_calls = AtomicUsize::new(0);
    let partition_calls = AtomicUsize::new(0);
    let policies = EnginePolicies::new(
        OtherContext {
            visits: &context_calls,
            label: "external-context".to_owned(),
        },
        OtherPartition(&partition_calls),
    );
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let fixture = rollback();
    let result = {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&algebra, &policies, &mut extension).run(
            AnalysisInput::new(rollback_world(&fixture), entry()).unwrap(),
            config(),
            &Control::default(),
        )
    }
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(context_calls.load(Ordering::Relaxed) > 0);
    assert!(partition_calls.load(Ordering::Relaxed) > 0);
    assert!(
        result
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return)
    );
    assert!(
        result
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .all(|outcome| !outcome
                .store
                .read(address(OWNER), &word(0), algebra.borrowed())
                .contains(U256::from(42)))
    );
}

#[path = "../support/root_input.rs"]
mod root_input;

#[test]
fn borrowed_nonclone_custom_algebra_retains_native_root_coverage() {
    let calls = Calls::default();
    let numeric = Policy::new(&calls);
    let values = evm_abstract::domain::ValueAlgebra::new(&numeric);
    root_input::assert_native_input_coverage(&values);
    assert!(calls.projections.load(std::sync::atomic::Ordering::Relaxed) > 0);
}
