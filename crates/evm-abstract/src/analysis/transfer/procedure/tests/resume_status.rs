//! Typed producer evidence routes relative resumes without numeric discovery.

use crate::{
    Fork, U256,
    analysis::{
        ExecutionConfig, MachineEdgeKind, OutcomeKind,
        engine::policy::PolicyContext,
        state::{
            EvmPayloadSummary,
            query::tests::carrier::{Algebra, Probe, Word},
        },
        transfer::{
            self, StorageReadPolicy,
            procedure::solver::{call_resume_status, executed_call},
        },
    },
    domain::{
        Domain, DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, NumericValue,
        ValueAlgebra, WordElement,
    },
    resource::WorkBudget,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use std::rc::Rc;

// The metadata-only carrier refuses numeric transfer by default. This fixture
// uses actual built-in EVM arithmetic, conservatively imported into its interval
// carrier, so real CALL/CREATE producers can run before the status is watched.
#[derive(Debug)]
struct StatusAlgebra;

fn numeric(value: &Word) -> NumericValue {
    NumericValue::from_observation(&value.observation())
}

impl EvmNumericAlgebra for StatusAlgebra {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        Algebra.spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        Algebra.project_numeric(value)
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        Algebra.join_numeric(left, right)
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        Algebra.widen_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        Algebra.widen_procedure_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        Algebra.reduce_numeric(value)
    }
    fn apply_numeric(&self, op: u8, args: &[Word]) -> NumericReduction<Word> {
        let args = args.iter().map(numeric).collect::<Vec<_>>();
        let result = Domain::default().apply_numeric(op, &args);
        NumericReduction {
            value: Word::from_observation(&result.value.observation()),
            status: result.status,
            rounds: result.rounds,
            strengthened: result.strengthened,
        }
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Word>) -> usize {
        let args = args.map(numeric).collect::<Vec<_>>();
        <Domain as EvmNumericAlgebra>::operation_work(&Domain::default(), args.iter())
    }
    fn literal_quote_work(&self, literal: U256) -> usize {
        Algebra.literal_quote_work(literal)
    }
    fn projection_work(&self, value: &Word) -> usize {
        Algebra.projection_work(value)
    }
    fn refinement_work(&self, value: &Word, facts: usize) -> usize {
        <Domain as EvmNumericAlgebra>::refinement_work(&Domain::default(), &numeric(value), facts)
    }
}

#[test]
fn relative_call_resumes_and_local_rejection_use_types_while_create_stays_unclassified() {
    let algebra = ValueAlgebra::new(StatusAlgebra);
    let config = ExecutionConfig::default();
    let owner = crate::analysis::machine::fixtures::address(0x101);
    let callee = crate::analysis::machine::fixtures::address(0x200);
    for creation in [false, true] {
        for completion in [
            None,
            Some(OutcomeKind::Return),
            Some(OutcomeKind::Revert),
            Some(OutcomeKind::Failure),
        ] {
            let mut world = EvmWorld::new(Fork::Osaka, "relative status producer evidence");
            let code = if creation {
                "5f5f5ff000"
            } else {
                "5f5f5f5f5f61020061fffff100"
            };
            world
                .insert(owner, EvmAccount::from_hex(code, world.fork()).unwrap())
                .unwrap();
            world
                .insert(callee, EvmAccount::from_hex("00", world.fork()).unwrap())
                .unwrap();
            if creation {
                // Missing offline accounts have unknown collision facts. The
                // real CREATE producer needs an established fresh destination.
                world.insert(owner.create(0), EvmAccount::absent()).unwrap();
            }
            let entry = EvmEntry::new(owner);
            let input = transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
            let mut budget = WorkBudget::new(1_000_000_000);
            let source = EvmPayloadSummary::project(
                &input,
                &mut PolicyContext::new(algebra.borrowed(), &entry, &mut budget),
            )
            .unwrap();
            let mut execution = transfer::execute_inner(
                &world,
                &entry,
                input,
                &config,
                algebra.borrowed(),
                &crate::analysis::RecentDistinctJumps,
                &mut budget,
                StorageReadPolicy::Abstract,
                true,
            );
            if let Some(kind) = completion {
                let child = execution
                    .successors
                    .into_iter()
                    .find(|successor| successor.kind == MachineEdgeKind::Call)
                    .expect("real producer entered a child")
                    .payload;
                execution = transfer::resume_summary_inner(
                    child,
                    kind,
                    EvmByteArray::empty(),
                    &config,
                    algebra.borrowed(),
                    &mut budget,
                    true,
                );
            } else {
                execution
                    .successors
                    .retain(|successor| successor.kind == MachineEdgeKind::Failure);
            }
            assert!(execution.frontiers.is_empty(), "{:?}", execution.frontiers);
            assert_eq!(execution.successors.len(), 1);
            let probe = Rc::new(Probe::new(usize::MAX));
            let value = execution.successors[0]
                .payload
                .active_mut()
                .stack
                .last_mut()
                .expect("the actual CALL/CREATE result");
            *value = EvmValue::from_numeric(value.numeric().clone().watched(&probe));
            let status = call_resume_status(
                executed_call(&source, &execution),
                source.key().frames.len(),
                &execution.completed_calls,
                &execution.successors[0],
            );
            let expected = (!creation).then_some(completion == Some(OutcomeKind::Return));
            assert_eq!(
                status, expected,
                "creation={creation}, completion={completion:?}"
            );
            assert_eq!(probe.counts(), (0, 0, 0));
            if !creation {
                let value = &execution.successors[0].payload.active().stack[0];
                assert!(value.contains(U256::from(u8::from(expected.unwrap()))));
            }
        }
    }
}
