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
        DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, ValueAlgebra, WordElement,
        concrete::{arity, evaluate},
    },
    resource::WorkBudget,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use std::rc::Rc;

// The metadata-only carrier refuses numeric transfer by default. This fixture
// evaluates exact words directly in the custom carrier, so real CALL/CREATE
// producers can run before the status is watched.
#[derive(Debug)]
struct StatusAlgebra;

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
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        if arity(op) != Some(args.len()) {
            return NumericReduction::stable(Word::top());
        }
        let mut words = [U256::ZERO; 3];
        for (word, argument) in words.iter_mut().zip(args) {
            let Some(exact) = argument.singleton() else {
                return NumericReduction::stable(Word::top());
            };
            *word = exact;
        }
        NumericReduction::stable(Word::constant(evaluate(op, words[0], words[1], words[2])))
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Word>) -> usize {
        // Own exact evaluation, a possible facade expression fold and its
        // fresh result query; quoting never exports or copies an operand.
        let evaluations = match op {
            revm_bytecode::opcode::EXP => 2 * 16_384,
            revm_bytecode::opcode::ADDMOD | revm_bytecode::opcode::MULMOD => 2 * 4096,
            revm_bytecode::opcode::DIV
            | revm_bytecode::opcode::SDIV
            | revm_bytecode::opcode::MOD
            | revm_bytecode::opcode::SMOD => 2 * 1024,
            _ => 2 * 64,
        };
        args.fold(1024usize + evaluations, |work, value| {
            work.saturating_add(value.singleton_work().saturating_mul(2))
        })
    }
    fn literal_quote_work(&self, literal: U256) -> usize {
        Algebra.literal_quote_work(literal)
    }
    fn projection_work(&self, value: &Word) -> usize {
        Algebra.projection_work(value)
    }
    fn refinement_work(&self, value: &Word, facts: usize) -> usize {
        // Word's native constraint import preserves hidden exclusions. Its
        // singleton check and the clone/reduce/project stages are all prepaid.
        value
            .clone_work()
            .saturating_mul(6)
            .saturating_add(
                value
                    .singleton_work()
                    .saturating_mul(facts.saturating_add(1)),
            )
            .saturating_add(facts.saturating_add(1).saturating_mul(1024))
    }
    fn summary_join_work(&self, l: &Word, r: &Word, w: bool) -> usize {
        Algebra.summary_join_work(l, r, w)
    }
    fn summary_canonical_work(&self, v: &Word) -> usize {
        Algebra.summary_canonical_work(v)
    }
    fn summary_construction_work(&self, c: crate::domain::SummaryConstruction) -> usize {
        Algebra.summary_construction_work(c)
    }
    fn summary_equal_work(&self, l: &Word, r: &Word) -> usize {
        Algebra.summary_equal_work(l, r)
    }
    fn summary_is_top_work(&self, v: &Word) -> usize {
        Algebra.summary_is_top_work(v)
    }
    fn summary_compact_work(&self, v: &Word) -> usize {
        Algebra.summary_compact_work(v)
    }
    fn summary_restore_work(&self, v: &Word, p: &Word) -> usize {
        Algebra.summary_restore_work(v, p)
    }
    fn summary_observation_work(&self, v: &crate::domain::WordObservation) -> usize {
        Algebra.summary_observation_work(v)
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
