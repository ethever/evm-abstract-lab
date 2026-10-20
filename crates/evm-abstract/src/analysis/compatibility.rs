//! Public composition checks across the engine boundary.

use super::{
    Arrival, ContextPolicy, ControlledAnalysisError, EnginePolicies, ExecutionConfig, OutcomeKind,
    PartitionPolicy, Status, analyze_world_with, analyze_world_with_components_control,
    control::{CancelToken, Control},
};
use crate::{
    Address, Fork, U256,
    domain::{
        AbstractValue, Domain, DomainSpec, EvmNumericAlgebra, NumericReduction, NumericValue,
        ValueAlgebra,
    },
    world::{Account, Entry, World},
};
use std::cell::Cell;

// These policies deliberately have neither Copy nor Clone. Borrowing must
// reach execution rather than just a public assembly constructor.
struct ContextProbe<'a> {
    calls: &'a Cell<usize>,
    cancel: Option<&'a CancelToken>,
}

impl ContextPolicy for ContextProbe<'_> {
    fn advance(&self, history: &mut Vec<usize>, _: usize, _: usize) {
        self.calls.set(self.calls.get() + 1);
        history.clear();
        if let Some(cancel) = self.cancel {
            cancel.cancel();
        }
    }

    fn normalize(&self, history: &mut Vec<usize>) {
        history.clear();
    }

    fn work(&self, length: usize) -> usize {
        length.saturating_add(1)
    }
}

struct PartitionProbe<'a>(&'a Cell<usize>);

impl PartitionPolicy for PartitionProbe<'_> {
    type Class = ();

    fn classify(&self, _: Arrival) {
        self.0.set(self.0.get() + 1);
    }
}

// NumericValue here tests the public policy boundary. Independent custom
// carriers are exercised by the cooperation suite; this probe specifically
// catches an entrypoint silently substituting its default numeric algebra.
#[derive(Debug)]
struct NumericProbe<'a> {
    domain: Domain,
    operations: &'a Cell<usize>,
    coarse_add: bool,
    empty_reduce: bool,
}

impl EvmNumericAlgebra for NumericProbe<'_> {
    type Element = NumericValue;

    fn spec(&self) -> DomainSpec {
        self.domain.spec()
    }

    fn project_numeric(&self, value: &NumericValue) -> NumericValue {
        EvmNumericAlgebra::project_numeric(&self.domain, value)
    }

    fn join_numeric(&self, left: &NumericValue, right: &NumericValue) -> NumericValue {
        EvmNumericAlgebra::join_numeric(&self.domain, left, right)
    }

    fn widen_numeric(&self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        EvmNumericAlgebra::widen_numeric(&self.domain, old, next)
    }

    fn widen_procedure_numeric(&self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        EvmNumericAlgebra::widen_procedure_numeric(&self.domain, old, next)
    }

    fn reduce_numeric(&self, value: &NumericValue) -> NumericReduction<NumericValue> {
        if self.empty_reduce {
            return NumericReduction {
                value: value.clone(),
                status: crate::domain::ReductionStatus::Empty,
                rounds: 0,
                strengthened: 0,
            };
        }
        EvmNumericAlgebra::reduce_numeric(&self.domain, value)
    }

    fn apply_numeric(&self, op: u8, args: &[NumericValue]) -> NumericReduction<NumericValue> {
        self.operations.set(self.operations.get() + 1);
        if self.coarse_add && op == revm_bytecode::opcode::ADD {
            return NumericReduction::stable(NumericValue::top());
        }
        EvmNumericAlgebra::apply_numeric(&self.domain, op, args)
    }

    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a,
    {
        EvmNumericAlgebra::operation_work(&self.domain, args)
    }

    fn projection_work(&self, value: &NumericValue) -> usize {
        EvmNumericAlgebra::projection_work(&self.domain, value)
    }

    fn refinement_work(&self, value: &NumericValue, facts: usize) -> usize {
        EvmNumericAlgebra::refinement_work(&self.domain, value, facts)
    }

    fn observe_byte_numeric(&self, op: u8, args: &[NumericValue]) -> NumericValue {
        EvmNumericAlgebra::observe_byte_numeric(&self.domain, op, args)
    }
}

fn fixture() -> (World, Entry) {
    let address = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "borrowed composition policies");
    // Push 42, jump, add 1 and return the result through memory.
    world
        .insert(
            address,
            Account::from_hex("602a600656005b6001015f5260205ff3", Fork::Osaka).unwrap(),
        )
        .unwrap();
    (world, Entry::new(address))
}

#[test]
fn borrowed_numeric_context_and_partition_policies_drive_production_execution() {
    let numeric_calls = Cell::new(0);
    let context_calls = Cell::new(0);
    let partition_calls = Cell::new(0);
    let graph = {
        let algebra = ValueAlgebra::new(NumericProbe {
            domain: Domain::default(),
            operations: &numeric_calls,
            coarse_add: false,
            empty_reduce: false,
        });
        let policies = EnginePolicies::new(
            ContextProbe {
                calls: &context_calls,
                cancel: None,
            },
            PartitionProbe(&partition_calls),
        );
        let (world, entry) = fixture();
        analyze_world_with(
            world,
            entry,
            ExecutionConfig::default(),
            &algebra,
            &policies,
        )
        .unwrap()
    };
    // Native results outlive the borrowed policy handles used for execution.
    assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
    assert!(
        numeric_calls.get() > 0,
        "injected numeric algebra was unused"
    );
    assert!(
        context_calls.get() > 0,
        "injected context policy was unused"
    );
    assert!(partition_calls.get() > 0, "injected partition was unused");
    assert!(graph.states().iter().all(|state| {
        state
            .key
            .frames
            .iter()
            .all(|frame| frame.jump_history.is_empty())
    }));
    let returns = graph
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect::<Vec<_>>();
    assert!(!returns.is_empty());
    for outcome in returns {
        assert_eq!(
            outcome
                .data
                .read_word(&AbstractValue::constant(U256::ZERO), Domain::default())
                .singleton(),
            Some(U256::from(43))
        );
    }
    // Both report adapters must preserve every existing default field. The
    // generic path serializes N directly, so custom domains use the same native
    // evidence augmentation without an execution conversion.
    assert_eq!(
        crate::render::world::json(&graph).unwrap(),
        crate::render::world::json_with(&graph).unwrap()
    );
}

#[test]
fn equal_domain_specs_do_not_share_results_between_numeric_instances() {
    // Disable expression constant recovery so the deliberately coarser ADD
    // implementation remains observable at this numeric injection boundary.
    let config = ExecutionConfig {
        analysis: super::Config {
            relations: crate::domain::relational::RelationLimits {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let domain = Domain::from_spec(config.analysis.validated_spec().unwrap());
    let policies: EnginePolicies = EnginePolicies::default();
    let (world, entry) = fixture();
    let mut observations = Vec::new();
    for coarse_add in [false, true, false] {
        let operations = Cell::new(0);
        let algebra = ValueAlgebra::new(NumericProbe {
            domain,
            operations: &operations,
            coarse_add,
            empty_reduce: false,
        });
        assert_eq!(algebra.spec(), domain.spec());
        let graph = analyze_world_with(
            world.clone(),
            entry.clone(),
            config.clone(),
            &algebra,
            &policies,
        )
        .unwrap();
        assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
        assert!(operations.get() > 0);
        let values = graph
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .map(|outcome| {
                outcome
                    .data
                    .read_word(&AbstractValue::constant(U256::ZERO), domain)
                    .singleton()
            })
            .collect::<Vec<_>>();
        assert!(!values.is_empty());
        assert!(values.iter().all(|value| {
            *value
                == if coarse_add {
                    None
                } else {
                    Some(U256::from(43))
                }
        }));
        observations.push(values);
    }
    assert_eq!(observations[0], observations[2]);
    assert_ne!(observations[0], observations[1]);
}

#[test]
fn custom_composition_cancellation_prevents_start_and_report_publication() {
    for cancel_before_start in [true, false] {
        let control = Control::new(super::progress::Observer::default());
        let context_calls = Cell::new(0);
        let partition_calls = Cell::new(0);
        let policies = EnginePolicies::new(
            ContextProbe {
                calls: &context_calls,
                cancel: Some(control.cancellation()),
            },
            PartitionProbe(&partition_calls),
        );
        let algebra = ValueAlgebra::new(Domain::default());
        let (world, entry) = fixture();
        if cancel_before_start {
            control.cancellation().cancel();
        }
        let result = analyze_world_with_components_control(
            world,
            entry,
            ExecutionConfig::default(),
            &algebra,
            &policies,
            &control,
        );
        assert!(matches!(result, Err(ControlledAnalysisError::Cancelled)));
        if cancel_before_start {
            assert_eq!(context_calls.get(), 0);
            assert_eq!(partition_calls.get(), 0);
        } else {
            assert!(context_calls.get() > 0);
        }
    }
}

#[test]
fn refused_optional_feedback_keeps_the_input_reachable_and_unmodified() {
    use super::cooperation::{LocalInput, StackFacts, apply_refinements};
    use crate::{
        domain::{ReductionStatus, WordObservation},
        resource::WorkBudget,
    };

    let operations = Cell::new(0);
    let algebra = ValueAlgebra::new(NumericProbe {
        domain: Domain::default(),
        operations: &operations,
        coarse_add: false,
        empty_reduce: true,
    });
    let mut payload = super::machine::fixtures::payload(Domain::default());
    payload.active_mut().stack.push(AbstractValue::top());
    let facts = StackFacts::at(0, WordObservation::constant(U256::from(7)));
    let result = apply_refinements(
        payload,
        &facts,
        algebra.borrowed(),
        &mut WorkBudget::new(20_000_000),
    )
    .unwrap();
    let LocalInput::Reachable { payload, limited } = result else {
        panic!("an optional Empty reduction is not an unreachable-input proof");
    };
    assert!(payload.active().stack[0].numeric().is_top());
    assert_eq!(limited, vec![ReductionStatus::Empty]);
}
