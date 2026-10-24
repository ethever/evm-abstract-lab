//! A real successful Runtime promotion followed by owned incoming revisits.

use super::super::coarse_retry::EvmRuntimeRetry as RuntimeRetry;
use super::*;
use crate::{
    analysis::{ExecutionConfig, state::stack_alias::StackAliases, worklist::runtime::TaskKind},
    domain::{EvmValue, ValueAlgebra},
    resource::clone_payment::tests::{
        Policy, RECEIPT, Receipt, SummaryPolicy, Word, reset, summary_fixture,
    },
    world::{EvmEntry, EvmWorld},
};

struct Harness {
    world: EvmWorld<Word>,
    entry: EvmEntry<Word>,
    config: ExecutionConfig,
    table: Table<SummaryPolicy>,
    counters: Counters,
}
impl Harness {
    fn new() -> Self {
        let (world, entry, mut config, algebra) = summary_fixture("525b00");
        config.use_summaries = true;
        let mut setup = WorkBudget::new(100_000_000);
        let mut input =
            crate::analysis::transfer::initial(&world, &entry, algebra.borrowed()).unwrap();
        let word = |value| EvmValue::<Word>::constant(U256::from(value));
        let byte = algebra.join(&word(1), &word(2));
        input
            .active_mut()
            .memory
            .write_byte(&word(0), &byte, algebra.borrowed())
            .unwrap();
        input.active_mut().stack = vec![word(7), word(0)];
        input.normalize();
        let summary = payload::caller::form_normalized_with_payment(
            &input,
            &entry,
            algebra.borrowed(),
            &mut setup,
        )
        .unwrap();
        let live =
            payload::caller::instantiate_with_payment(&summary, algebra.borrowed(), &mut setup)
                .unwrap();
        // The actual MSTORE refuses its retained byte root. A successful retry
        // installs the persistent mode; no test writes runtime_retry itself.
        let mut relations = algebra.spec().relations();
        relations.max_nodes = live.active().memory.state_root().nodes() + 2;
        let algebra = ValueAlgebra::new(SummaryPolicy(Policy(ProductAlgebra::from_spec(
            algebra.spec().with_relations(relations),
        ))));
        let mut table = Table::with_algebra(algebra);
        let mut counters = Counters::default();
        let mut solver = table.solver(
            &world,
            &entry,
            &config,
            &mut setup,
            &mut counters,
            StorageReadPolicy::Abstract,
        );
        assert_eq!(solver.procedure(input).unwrap(), 0);
        let original = solver.table.nodes[0]
            .payload
            .clone_with_payment(solver.budget)
            .unwrap();
        let live =
            payload::caller::instantiate_with_payment(&original, solver.domain, solver.budget)
                .unwrap();
        solver.counters.transfers += 1;
        let first = execute_inner_with(
            solver.world,
            solver.entry,
            live,
            solver.config,
            solver.domain,
            &solver.policies.context,
            solver.budget,
            solver.storage_reads,
            true,
        );
        assert_eq!(first.retention_failures.len(), 1);
        let retried = solver.retry_runtime(0, &original, first);
        assert!(retried.frontiers.is_empty(), "{:?}", retried.frontiers);
        assert!(matches!(
            solver.table.nodes[0].runtime_retry,
            RuntimeRetry::Coarse(_)
        ));
        assert_eq!(solver.counters.transfers, 2);
        Self {
            world,
            entry,
            config,
            table,
            counters,
        }
    }

    fn retained_copy(&self) -> EvmPayloadSummary<Word> {
        self.table.nodes[0]
            .payload
            .clone_with_payment(&mut WorkBudget::new(100_000_000))
            .unwrap()
    }

    fn incoming(&self, word: Word) -> EvmPayloadSummary<Word> {
        // Fixture copying is separately paid before installing the test's
        // independent tariff and before resetting its observed ledger/hooks.
        let mut incoming = self.retained_copy();
        incoming.frames[0].stack[0] = EvmValue::from_numeric(word);
        incoming.frames[0].stack_aliases =
            StackAliases::form(&incoming.frames[0].stack, true).unwrap();
        incoming.work = incoming.measure_work();
        incoming
    }

    fn revisit(
        &mut self,
        incoming: EvmPayloadSummary<Word>,
        budget: &mut WorkBudget,
    ) -> Result<usize, MachineError> {
        self.table
            .solver(
                &self.world,
                &self.entry,
                &self.config,
                budget,
                &mut self.counters,
                StorageReadPolicy::Abstract,
            )
            .enqueue_summary(0, incoming, None, None)
    }

    fn assert_pending(&self) {
        assert!(
            self.table
                .runtime
                .pending_relative()
                .any(|task| task == TaskKind::Body(0)),
            "the real native Body obligation remains queued"
        );
        assert_ne!(
            self.table.runtime.view().closure(),
            analysis_kernel::Closure::Closed
        );
        assert!(matches!(
            self.table.nodes[0].runtime_retry,
            RuntimeRetry::Coarse(_)
        ));
        assert_eq!(self.counters.transfers, 2);
    }

    fn produced_successor(&self) -> EvmExecution<Word> {
        // Supply an owned successor at the admission seam. Its earlier
        // production/replay belongs to setup, not this accept/revisit ledger.
        let mut setup = WorkBudget::new(100_000_000);
        let incoming = self.incoming(Word::expensive(1_000_000));
        let payload =
            payload::caller::instantiate_with_payment(&incoming, self.table.algebra(), &mut setup)
                .unwrap();
        payload.pay_clone_with(&mut setup).unwrap();
        let mut execution = super::super::super::empty_execution(payload.clone());
        execution.successors.push(EvmSuccessor {
            payload,
            kind: MachineEdgeKind::Intraprocedural(crate::analysis::EdgeKind::Jump),
        });
        execution
    }

    fn accept(&mut self, execution: EvmExecution<Word>, budget: &mut WorkBudget) {
        self.table
            .solver(
                &self.world,
                &self.entry,
                &self.config,
                budget,
                &mut self.counters,
                StorageReadPolicy::Abstract,
            )
            .accept(0, execution);
    }
}

#[test]
fn persistent_coarse_revisit_charges_native_copy_and_retains_its_closure() {
    let _scope = RECEIPT.scope();
    let mut costs = Vec::new();
    for fee in [64, 1_000_000] {
        let mut h = Harness::new();
        let layout = match &h.table.nodes[0].runtime_retry {
            RuntimeRetry::Coarse(coarse) => coarse
                .layout()
                .clone_with_payment(&mut WorkBudget::new(100_000_000))
                .unwrap(),
            _ => unreachable!(),
        };
        let mut budget = WorkBudget::new(100_000_000);
        let incoming = h.incoming(Word::expensive(fee));
        reset();
        assert_eq!(h.revisit(incoming, &mut budget).unwrap(), 0);
        assert_eq!(RECEIPT.get().bytes, 4096);
        assert_eq!(
            RECEIPT.get().private_quotes,
            1,
            "the incoming value is paid before coarse erasure"
        );
        costs.push(budget.used());
        let updates = h.table.nodes[0].updates;
        // This existing test carrier exposes the conservative default interval
        // Top. Its persistent coarse closure therefore covers all these words,
        // including the erased private exclusion, rather than claiming [0,32].
        for value in [0, 7, 8, 32, u64::MAX] {
            assert!(h.table.nodes[0].payload.active().stack[0].contains(U256::from(value)));
        }
        for _ in 0..2 {
            let incoming = h.incoming(Word::expensive(fee));
            h.revisit(incoming, &mut budget).unwrap();
            let node = &h.table.nodes[0];
            assert_eq!(node.updates, updates, "duplicate arrival stays stable");
            assert_eq!(node.payload.storage_observations, Default::default());
            assert!(!node.payload.active().stack_aliases.same_class(0, 1));
            let RuntimeRetry::Coarse(coarse) = &node.runtime_retry else {
                panic!("mode reset")
            };
            assert!(semantic_eq(&layout, coarse.layout()));
        }
        assert!(!budget.exhausted());
        h.assert_pending();
    }
    assert_eq!(costs[1] - costs[0], 1_000_000 - 64);
}

#[test]
fn persistent_coarse_revisit_refuses_copy_before_hook_without_replacing_bound() {
    let _scope = RECEIPT.scope();
    for fee in [1_000_000, usize::MAX] {
        let mut h = Harness::new();
        let before = h.retained_copy();
        let updates = h.table.nodes[0].updates;
        let incoming = h.incoming(Word::expensive(fee));
        let mut budget = WorkBudget::new(500_000);
        reset();
        assert_eq!(h.revisit(incoming, &mut budget), Err(MachineError::Work));
        assert!(
            RECEIPT.get().private_quotes > 0,
            "reached persistent coarse native invoice"
        );
        assert_eq!(
            RECEIPT.get().bytes,
            0,
            "refused private value was never cloned"
        );
        assert!(semantic_eq(&before, &h.table.nodes[0].payload));
        assert_eq!(h.table.nodes[0].updates, updates);
        h.assert_pending();
    }
}

#[test]
fn persistent_coarse_revisit_checks_both_cancellation_owners_before_hook() {
    let _scope = RECEIPT.scope();
    for captured_target in [true, false] {
        let mut h = Harness::new();
        let before = h.retained_copy();
        let captured = analysis_control::Cancellation::new();
        let ambient = analysis_control::Cancellation::new();
        let mut word = Word::expensive(64);
        word.cancel_price = Some(if captured_target {
            captured.clone()
        } else {
            ambient.clone()
        });
        let incoming = h.incoming(word);
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(100_000_000));
        RECEIPT.set(Receipt {
            armed: true,
            ..Receipt::default()
        });
        assert_eq!(
            analysis_control::with_cancellation(&ambient, || h.revisit(incoming, &mut budget)),
            Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        );
        assert!(RECEIPT.get().private_quotes > 0);
        assert_eq!(RECEIPT.get().bytes, 0);
        assert!(semantic_eq(&before, &h.table.nodes[0].payload));
        h.assert_pending();
    }
}

#[test]
fn accept_preserves_the_original_successor_when_its_coarse_copy_is_refused() {
    use crate::test_instrumentation::work_boundary::first_work_reaching;

    let _scope = RECEIPT.scope();
    let funded = 100_000_000;
    let mut h = Harness::new();
    let execution = h.produced_successor();
    reset();
    h.accept(execution, &mut WorkBudget::new(funded));
    assert!(h.table.frontiers.is_empty());
    let paid = RECEIPT.get();
    assert!(paid.private_quotes > 0 && paid.bytes >= 4096);
    // Owned partitioning does no native copy. The separate direct revisit
    // regression proves that this final private quote belongs to Coarse's
    // incoming copy; normalization erases it before later policy admission.
    let cut = first_work_reaching(funded, |limit| {
        let mut h = Harness::new();
        let execution = h.produced_successor();
        reset();
        h.accept(execution, &mut WorkBudget::new(limit));
        RECEIPT.get().private_quotes == paid.private_quotes
    });
    let mut h = Harness::new();
    let before = h.retained_copy();
    let execution = h.produced_successor();
    let mut budget = WorkBudget::new(cut);
    reset();
    h.accept(execution, &mut budget);
    assert_eq!(
        RECEIPT.get().private_quotes,
        paid.private_quotes,
        "the Coarse invoice was reached"
    );
    assert_eq!(
        RECEIPT.get().bytes + 4096,
        paid.bytes,
        "only the refused private clone is absent"
    );
    assert!(semantic_eq(&before, &h.table.nodes[0].payload));
    assert_eq!(h.table.frontiers.len(), 1);
    let frontier = &h.table.frontiers[0];
    assert_eq!(frontier.reason, FrontierReason::Work);
    let input = frontier
        .payload
        .as_ref()
        .expect("whole incoming obligation retained");
    assert_eq!(frontier.target.as_ref(), Some(&input.key()));
    assert!(!input.active().stack[0].contains(U256::from(7)));
    assert!(input.active().stack[0].contains(U256::from(8)));
    h.assert_pending();
    eprintln!(
        "Coarse accept boundary: limit={cut}, private quotes={}, paid bytes={}, refused bytes={}",
        paid.private_quotes,
        paid.bytes,
        RECEIPT.get().bytes
    );
}
