//! Native admission, transfer and delivery evidence must remain distinct.

use super::super::{
    ByteArray, Counters, StorageReadPolicy, Table, TestValue, WorkBudget, testing::initial,
};
use super::*;
use crate::analysis::{
    EdgeKind, FrameCode, ProcedureSummaryRecord,
    machine::fixtures::{address, config, fixture},
    testing::{NumericLimits, product},
};
use crate::world::AddressInput;
use std::collections::BTreeSet;

// These one-shot, thread-scoped probes cover otherwise global identity exhaustion
// and a precise cancellation interleaving. They never alter semantic payloads or
// publication versions and are absent from non-test builds.
static BODY_REFUSAL: crate::test_instrumentation::Counter<(bool, usize)> =
    crate::test_instrumentation::Counter::new((false, 0));
static EXPORT_CANCELLATION: crate::test_instrumentation::Counter<(Option<bool>, usize)> =
    crate::test_instrumentation::Counter::new((None, 0));

pub(in crate::analysis::transfer::procedure) fn refuse_body_instantiation() -> Option<MachineError>
{
    let mut refuse = false;
    BODY_REFUSAL.update(|(armed, hits)| {
        refuse = std::mem::take(armed);
        *hits += usize::from(refuse);
    });
    refuse.then_some(MachineError::Bytes(crate::world::RangeError::State(
        crate::domain::symbolic::StateError::IdentityExhausted,
    )))
}

pub(in crate::analysis::transfer::procedure) fn cancel_after_export_census(budget: &WorkBudget) {
    let mut target = None;
    EXPORT_CANCELLATION.update(|(armed, hits)| {
        target = armed.take();
        *hits += usize::from(target.is_some());
    });
    match target {
        Some(true) => budget.cancellation().cancel(),
        Some(false) => analysis_control::current_cancellation().cancel(),
        None => {}
    }
}

#[test]
fn same_version_body_preparation_refusal_does_not_recertify_the_previous_transfer() {
    use crate::analysis::{FrontierReason, JoinBoundary, worklist::MachineMeter};
    use std::sync::Arc;

    let _scope = BODY_REFUSAL.scope();
    let (world, mut entry) = fixture(&[(0x101, "00")]);
    entry.environment.caller = AddressInput::unknown_caller();
    entry.environment.value = TestValue::top();
    entry.environment.calldata = ByteArray::unknown();
    let config = config();
    let spec = NumericLimits::machine().spec();
    let algebra = product(spec);
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let mut table = Table::with_algebra(product(spec));
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    let procedure = solver.procedure(input).unwrap();
    let body = solver.table.procedures[procedure].first_body();
    let (task, mut token) = solver.runtime.next(true, solver.budget).unwrap().unwrap();
    assert_eq!(task.0, TaskKind::Body(body));
    let job = solver
        .runtime
        .with_run(&mut token, solver.budget, |run| Ok(run.job()))
        .unwrap();
    assert!(solver.run_admitted(Arc::clone(&task), token));
    let before = solver.regional_cfg().unwrap();
    let old = before.nodes[body].transfer.as_ref().unwrap();
    assert!(old.current && old.complete && old.visited_block);
    let input_version = solver.track_input(body).unwrap();
    let admitted = solver.table.nodes[body].payload.clone();
    let transfers = solver.counters.transfers;
    let frontiers = solver.table.frontiers.len();

    // Retry the SAME real native job. No payload mutation/publication or version
    // growth is used to manufacture the preparation-refusal counterexample.
    solver
        .runtime
        .kernel()
        .retry(job, &mut MachineMeter(solver.budget))
        .unwrap();
    let (retry, token) = solver.runtime.next(true, solver.budget).unwrap().unwrap();
    assert_eq!(retry.0, task.0);
    BODY_REFUSAL.set((true, 0));
    assert!(solver.run_admitted(retry, token));
    assert_eq!(BODY_REFUSAL.get(), (false, 1));
    assert_eq!(solver.counters.transfers, transfers);
    assert_eq!(solver.track_input(body).unwrap(), input_version);
    assert_eq!(solver.table.nodes[body].payload, admitted);
    assert!(!solver.budget.exhausted());
    assert_eq!(solver.table.frontiers.len(), frontiers + 1);
    assert_eq!(
        solver.table.frontiers.last().unwrap().reason,
        FrontierReason::StateJoin(JoinBoundary::IdentityExhausted)
    );
    let after = solver.regional_cfg().unwrap();
    let receipt = after.nodes[body].transfer.as_ref().unwrap();
    assert_eq!(receipt.input_revision, old.input_revision);
    assert_eq!(receipt.outcomes, old.outcomes);
    assert!(
        receipt.complete,
        "historical completeness remains historical"
    );
    assert!(
        !receipt.current,
        "a refused evaluation installed no new transfer"
    );
    // The same admitted input can subsequently execute for real. Its fresh
    // receipt is current again, while the earlier global frontier is retained.
    solver
        .runtime
        .kernel()
        .retry(job, &mut MachineMeter(solver.budget))
        .unwrap();
    let (retry, token) = solver.runtime.next(true, solver.budget).unwrap().unwrap();
    assert!(solver.run_admitted(retry, token));
    assert_eq!(solver.counters.transfers, transfers + 1);
    assert_eq!(solver.track_input(body).unwrap(), input_version);
    assert_eq!(solver.table.frontiers.len(), frontiers + 1);
    let recovered = solver.regional_cfg().unwrap();
    let recovered = recovered.nodes[body].transfer.as_ref().unwrap();
    assert!(recovered.current && recovered.complete && recovered.visited_block);
    eprintln!(
        "E1 controlled: revision={}, refused transfers {} -> {}, real rerun -> {}; retained frontiers={}",
        input_version.get(),
        transfers,
        transfers,
        solver.counters.transfers,
        solver.table.frontiers.len(),
    );
}

#[test]
fn cancellation_after_export_census_uses_the_correct_owner_stop_reason() {
    use crate::domain::relational::QueryReason;
    use analysis_control::{Cancellation, with_cancellation};

    let _scope = EXPORT_CANCELLATION.scope();
    for captured_target in [true, false] {
        let captured = Cancellation::new();
        let ambient = Cancellation::new();
        let (world, entry) = fixture(&[(0x101, "00")]);
        let config = config();
        let spec = NumericLimits::machine().spec();
        let algebra = product(spec);
        let input = initial(&world, &entry, algebra.borrowed()).unwrap();
        let mut table = Table::with_algebra(product(spec));
        let mut budget = with_cancellation(&captured, || WorkBudget::new(config.max_work));
        let mut counters = Counters::default();
        let mut solver = table.solver(
            &world,
            &entry,
            &config,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        );
        solver.procedure(input).unwrap();
        solver.run();
        assert!(!captured.is_cancelled() && !ambient.is_cancelled());
        let before = solver.budget.used();
        EXPORT_CANCELLATION.set((Some(captured_target), 0));
        let result = with_cancellation(&ambient, || solver.regional_cfg());
        assert_eq!(
            EXPORT_CANCELLATION.get(),
            (None, 1),
            "passed initial checkpoint and census"
        );
        assert_eq!(captured.is_cancelled(), captured_target);
        assert_eq!(ambient.is_cancelled(), !captured_target);
        assert_eq!(
            solver.budget.used(),
            before,
            "cancelled quote cannot publish/pay copies"
        );
        assert_eq!(
            result,
            Err(MachineError::Relations(QueryReason::Cancelled)),
            "captured_target={captured_target}"
        );
    }
}

#[test]
fn duplicate_resume_keeps_real_return_revert_and_failure_delivery_receipts() {
    use crate::analysis::worklist::MachineMeter;

    for (child, outcome) in [
        ("00", OutcomeKind::Return),
        ("5f5ffd", OutcomeKind::Revert),
        ("fe", OutcomeKind::Failure),
    ] {
        let (world, mut entry) = fixture(&[(0x101, "5f5f5f5f5f6101025af15000"), (0x102, child)]);
        entry.environment.caller = AddressInput::unknown_caller();
        entry.environment.value = TestValue::top();
        entry.environment.calldata = ByteArray::unknown();
        let config = config();
        let spec = NumericLimits::machine().spec();
        let algebra = product(spec);
        let input = initial(&world, &entry, algebra.borrowed()).unwrap();
        let mut table = Table::with_algebra(product(spec));
        let mut budget = WorkBudget::new(config.max_work);
        let mut counters = Counters::default();
        let mut solver = table.solver(
            &world,
            &entry,
            &config,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        );
        solver.procedure(input).unwrap();
        let mut resume = None;
        while let Some((task, mut token)) = solver.runtime.next(true, solver.budget).unwrap() {
            if let TaskKind::Resume(id) = &task.0 {
                let job = solver
                    .runtime
                    .with_run(&mut token, solver.budget, |run| Ok(run.job()))
                    .unwrap();
                resume = Some((*id, job));
            }
            assert!(solver.run_admitted(task, token));
        }
        assert!(solver.table.frontiers.is_empty());
        assert!(solver.runtime.relative_closed(solver.budget).unwrap());
        let (subscription, job) = resume.expect("the real CALL must have a Resume job");
        let before = solver.regional_cfg().unwrap();
        let returned = before.calls[subscription]
            .deliveries
            .iter()
            .find(|receipt| receipt.kind == outcome)
            .unwrap();
        assert!(returned.current && !returned.successors.is_empty());
        let expected_edge = match outcome {
            OutcomeKind::Return => MachineEdgeKind::Return,
            OutcomeKind::Revert => MachineEdgeKind::Revert,
            OutcomeKind::Failure => MachineEdgeKind::Failure,
        };
        assert!(
            returned
                .successors
                .iter()
                .all(|edge| edge.kind == expected_edge)
        );
        let stamps = solver.table.subscribers[subscription].delivered.clone();
        let propagations = solver.table.propagations;
        let transfers = solver.counters.transfers;

        solver
            .runtime
            .kernel()
            .retry(job, &mut MachineMeter(solver.budget))
            .unwrap();
        let (retry, token) = solver.runtime.next(true, solver.budget).unwrap().unwrap();
        assert_eq!(retry.0, TaskKind::Resume(subscription));
        assert!(solver.run_admitted(retry, token));
        let after = solver.regional_cfg().unwrap();
        assert_eq!(solver.counters.transfers, transfers);
        assert_eq!(
            solver.table.propagations, propagations,
            "unchanged native stamps skipped actual redelivery"
        );
        assert_eq!(solver.table.subscribers[subscription].delivered, stamps);
        assert_eq!(
            after.calls[subscription], before.calls[subscription],
            "{outcome:?}"
        );
        assert!(solver.table.frontiers.is_empty());
    }
}

/// Exercise the production relative table directly, without an outer terminal
/// envelope. Caller, value and calldata remain unspecified in these fixtures.
fn report(accounts: &[(u64, &str)]) -> ProcedureSummaryRecord {
    let (world, mut entry) = fixture(accounts);
    entry.environment.caller = AddressInput::unknown_caller();
    entry.environment.value = TestValue::top();
    entry.environment.calldata = ByteArray::unknown();
    let config = config();
    let spec = NumericLimits::machine().spec();
    let algebra = product(spec);
    let domain = algebra.borrowed();
    let input = initial(&world, &entry, domain).unwrap();
    let mut table = Table::with_algebra(product(spec));
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let (execution, record) = table.execute(
        0,
        &world,
        &entry,
        input,
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(record.closed, "{:?}", execution.frontiers);
    assert!(execution.frontiers.is_empty());
    assert_indices(&record);
    record
}

fn assert_indices(record: &ProcedureSummaryRecord) {
    let cfg = &record.regional_cfg;
    for (id, node) in cfg.nodes.iter().enumerate() {
        assert_eq!(node.id, id);
        assert!(node.procedure < record.procedures.len());
        assert!(node.successors.iter().all(|id| *id < cfg.nodes.len()));
        if let Some(receipt) = &node.transfer {
            assert!(
                receipt
                    .successors
                    .iter()
                    .all(|edge| edge.to < cfg.nodes.len())
            );
            assert!(receipt.calls.iter().all(|id| *id < cfg.calls.len()));
        }
    }
    for (id, call) in cfg.calls.iter().enumerate() {
        assert_eq!(call.id, id);
        assert!(call.from < cfg.nodes.len() && call.callee < record.procedures.len());
        assert!(
            call.entries
                .iter()
                .all(|id| { *id < cfg.nodes.len() && cfg.nodes[*id].procedure == call.callee })
        );
        assert!(call.deliveries.iter().all(|receipt| {
            receipt
                .successors
                .iter()
                .all(|edge| edge.to < cfg.nodes.len())
        }));
    }
    for procedure in &record.procedures {
        assert!(
            procedure
                .dependencies
                .iter()
                .all(|id| *id < record.procedures.len())
        );
    }
}

#[test]
fn pending_nodes_and_equal_numeric_feedback_do_not_certify_execution() {
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = config();
    let spec = NumericLimits::machine().spec();
    let algebra = product(spec);
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let mut table = Table::with_algebra(product(spec));
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    let procedure = solver.procedure(input).unwrap();
    let body = solver.table.procedures[procedure].first_body();
    let pending = solver.regional_cfg().unwrap();
    assert_eq!(pending.nodes.len(), 1);
    assert!(pending.nodes[body].transfer.is_none());

    solver.run();
    let executed = solver.regional_cfg().unwrap();
    let receipt = executed.nodes[body].transfer.as_ref().unwrap();
    assert!(receipt.current && receipt.complete && receipt.visited_block);
    assert_eq!(receipt.outcomes, vec![OutcomeKind::Return]);
    let revision = executed.nodes[body].input_revision;
    let updates = solver.table.nodes[body].updates;
    let bound = solver.table.nodes[body].payload.clone();

    // Loop-head feedback publishes a semantic scheduling change even though
    // the numerical bound and strict numeric-update count remain unchanged.
    solver.admit_node(body, &bound, false, true, None).unwrap();
    assert_eq!(solver.table.nodes[body].updates, updates);
    let invalidated = solver.regional_cfg().unwrap();
    assert!(invalidated.nodes[body].input_revision > revision);
    assert!(!invalidated.nodes[body].transfer.as_ref().unwrap().current);
    solver.run();
    let refreshed = solver.regional_cfg().unwrap();
    assert!(refreshed.nodes[body].transfer.as_ref().unwrap().current);

    // A retry invalidates its previous receipt before any fallible preparation;
    // an equal native version cannot certify a failed/deferred reevaluation.
    solver.regional_begin(&TaskKind::Body(body));
    let interrupted = solver.regional_cfg().unwrap();
    assert_eq!(
        interrupted.nodes[body].input_revision,
        refreshed.nodes[body].input_revision
    );
    assert!(!interrupted.nodes[body].transfer.as_ref().unwrap().current);
}

#[test]
fn actual_dynamic_jump_edges_keep_both_targets_and_exclude_a_legal_decoy() {
    // Dispatch at PC24 receives {26,49}; PC41 is a legal JUMPDEST between them.
    // The Unknown input is preserved; no concrete witness narrows the analysis.
    let record = report(&[(
        0x101,
        "610000351561001057610031610018565b61001a610018565b565b61000b61000052610020610000f35b610000610000fd5b61001661000052610020610000f3",
    )]);
    let cfg = &record.regional_cfg;
    let program = crate::bytecode::Program::from_hex_with_fork(
        "610000351561001057610031610018565b61001a610018565b565b61000b61000052610020610000f35b610000610000fd5b61001661000052610020610000f3",
        crate::Fork::Osaka,
    )
    .unwrap();
    let block_at = |index: usize| program.blocks()[index].start_pc;
    let dispatch: Vec<_> = cfg
        .nodes
        .iter()
        .filter(|node| block_at(node.frame.basic_block_index) == 24)
        .collect();
    assert!(!dispatch.is_empty());
    let targets: BTreeSet<_> = dispatch
        .iter()
        .flat_map(|node| {
            let transfer = node.transfer.as_ref().unwrap();
            assert!(transfer.current && transfer.complete && transfer.visited_block);
            transfer.successors.iter().map(|edge| {
                assert_eq!(edge.kind, MachineEdgeKind::Intraprocedural(EdgeKind::Jump));
                block_at(cfg.nodes[edge.to].frame.basic_block_index)
            })
        })
        .collect();
    assert_eq!(targets, BTreeSet::from([26, 49]));
    assert!(cfg.nodes.iter().all(|node| {
        block_at(node.frame.basic_block_index) != 41
            || !node
                .transfer
                .as_ref()
                .is_some_and(|receipt| receipt.current && receipt.visited_block)
    }));
}

#[test]
fn actual_calls_keep_owner_caller_mode_and_their_return_continuations() {
    let record = report(&[
        (
            0x101,
            concat!(
                "5f5f5f5f5f6101025af150",   // CALL
                "5f5f5f5f6101025af450",     // DELEGATECALL
                "5f5f5f5f60036101025af250", // CALLCODE
                "5f5f5f5f6101025afa5000",   // STATICCALL, STOP
            ),
        ),
        (0x102, "00"),
    ]);
    let cfg = &record.regional_cfg;
    let children: Vec<_> = cfg
        .nodes
        .iter()
        .filter(|node| {
            node.frame.code_address == address(0x102)
                && node.transfer.as_ref().is_some_and(|receipt| {
                    receipt.current && receipt.complete && receipt.visited_block
                })
        })
        .collect();
    for (owner, caller, is_static) in [
        (0x102, address(0x101).into(), false),
        (0x101, AddressInput::unknown_caller(), false),
        (0x101, address(0x101).into(), false),
        (0x102, address(0x101).into(), true),
    ] {
        assert!(children.iter().any(|node| {
            node.frame.address == address(owner)
                && node.frame.caller == caller
                && node.frame.is_static == is_static
                && node.frame.mode == FrameCode::Runtime
        }));
    }
    assert!(children.iter().all(|node| {
        node.frame.code_hash == children[0].frame.code_hash
            && node.frame.code_address != cfg.nodes[0].frame.code_address
    }));
    assert!(!cfg.calls.is_empty());
    for call in &cfg.calls {
        let source = &cfg.nodes[call.from];
        assert_eq!(call.id, cfg.calls[call.id].id);
        assert!(source.transfer.as_ref().is_some_and(|receipt| {
            receipt.current && receipt.complete && receipt.calls.contains(&call.id)
        }));
        assert!(!call.entries.is_empty());
        assert!(
            call.entries
                .iter()
                .all(|id| cfg.nodes[*id].procedure == call.callee)
        );
        let returned = call
            .deliveries
            .iter()
            .find(|delivery| delivery.current && delivery.kind == OutcomeKind::Return)
            .expect("a real callee STOP was delivered");
        assert!(!returned.successors.is_empty());
        for edge in &returned.successors {
            assert_eq!(edge.kind, MachineEdgeKind::Return);
            assert_eq!(cfg.nodes[edge.to].procedure, source.procedure);
            assert!(source.successors.contains(&edge.to));
            assert!(!source.transfer.as_ref().unwrap().successors.contains(edge));
        }
    }
}

#[test]
fn unknown_processing_has_no_invented_visited_bytecode_block() {
    let record = report(&[]);
    assert!(!record.regional_cfg.nodes.is_empty());
    assert!(record.regional_cfg.nodes.iter().all(|node| {
        node.frame.mode == FrameCode::Unknown
            && node.transfer.as_ref().is_some_and(|receipt| {
                receipt.current && receipt.complete && !receipt.visited_block
            })
    }));
}

#[test]
fn delivery_targets_require_commit_and_both_current_native_stamps() {
    let input = Version::default();
    let mut receipt = ReturnReceipt {
        report: EvmRegionalReturn {
            creation: Vec::new(),
            kind: OutcomeKind::Revert,
            input_revision: input.get(),
            output_generation: 7,
            current: true,
            successors: vec![EvmRegionalEdge {
                to: 3,
                kind: MachineEdgeKind::Revert,
            }],
        },
        delivered: false,
    };
    assert!(
        receipt
            .snapshot(input, Some(7), Some((input, 7)), true)
            .is_none()
    );
    receipt.delivered = true;
    let current = receipt
        .snapshot(input, Some(7), Some((input, 7)), true)
        .unwrap();
    assert!(current.current);
    assert_eq!(current.successors[0].kind, MachineEdgeKind::Revert);
    assert!(
        !receipt
            .snapshot(input, Some(7), Some((input, 7)), false)
            .unwrap()
            .current
    );
    for (generation, delivered) in [
        (Some(8), Some((input, 7))),
        (Some(7), Some((input, 8))),
        (Some(7), None),
    ] {
        assert!(
            !receipt
                .snapshot(input, generation, delivered, true)
                .unwrap()
                .current
        );
    }
    receipt.report.input_revision += 1;
    assert!(
        !receipt
            .snapshot(input, Some(7), Some((input, 7)), true)
            .unwrap()
            .current
    );
}

#[test]
fn export_checks_captured_and_current_cancellation_and_does_not_revive_a_spent_ledger() {
    use crate::domain::relational::QueryReason;
    use analysis_control::{Cancellation, with_cancellation};

    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = config();
    let spec = NumericLimits::machine().spec();
    let algebra = product(spec);
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let captured = Cancellation::new();
    let mut budget = with_cancellation(&captured, || WorkBudget::new(config.max_work));
    let mut counters = Counters::default();
    let mut table = Table::with_algebra(product(spec));
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    solver.procedure(input.clone()).unwrap();
    solver.run();
    let cfg = solver.regional_cfg().unwrap();
    assert!(!cfg.nodes.is_empty());

    let ambient = Cancellation::new();
    ambient.cancel();
    let before = solver.budget.used();
    assert!(matches!(
        with_cancellation(&ambient, || solver.regional_cfg()),
        Err(MachineError::Relations(QueryReason::Cancelled))
    ));
    assert_eq!(solver.budget.used(), before);
    assert!(!solver.budget.exhausted());
    assert!(solver.regional_cfg().is_ok());

    // The final zero-work checkpoint must still reject a prior refused charge.
    assert!(!solver.budget.charge(usize::MAX));
    assert!(matches!(
        solver.regional_checkpoint(),
        Err(MachineError::Work)
    ));
    captured.cancel();
    assert!(matches!(
        solver.regional_cfg(),
        Err(MachineError::Relations(QueryReason::Cancelled))
    ));
    assert!(matches!(
        solver.regional_checkpoint(),
        Err(MachineError::Relations(QueryReason::Cancelled))
    ));
    let (execution, record) = table.execute(
        0,
        &world,
        &entry,
        input,
        &config,
        algebra.borrowed(),
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(!record.closed);
    assert!(record.regional_cfg.nodes.is_empty() && record.regional_cfg.calls.is_empty());
    assert!(execution.frontiers.iter().any(|frontier| {
        frontier.reason == crate::analysis::FrontierReason::Relations(QueryReason::Cancelled)
    }));
}

#[test]
fn cooperative_exports_withhold_current_even_for_complete_matching_native_receipts() {
    use super::super::{Cooperation, EvmTable};
    use crate::analysis::{
        EnginePolicies,
        cooperation::{Extension, Subject},
        worklist::runtime::Runtime,
    };
    use analysis_kernel::{Analyzer, LocalClosure};
    use std::convert::Infallible;

    struct Observe;
    impl<Cx> Analyzer<Cx> for Observe {
        type Task = Subject;
        type Error = Infallible;
        fn evaluate(&mut self, _: &Subject, _: &mut Cx) -> Result<LocalClosure, Infallible> {
            Ok(LocalClosure::Stable)
        }
    }

    let (world, mut entry) = fixture(&[(0x101, "5f5f5f5f5f6101025af15000"), (0x102, "00")]);
    entry.environment.caller = AddressInput::unknown_caller();
    entry.environment.value = TestValue::top();
    entry.environment.calldata = ByteArray::unknown();
    let config = config();
    let algebra = product(NumericLimits::machine().spec());
    let domain = algebra.borrowed();
    let input = initial(&world, &entry, domain).unwrap();
    let mut budget = WorkBudget::new(config.max_work);
    let mut runtime = Runtime::new(&mut budget).unwrap();
    runtime.register_world(0, 1, &mut budget).unwrap();
    let (_, mut caller) = runtime.next(false, &mut budget).unwrap().unwrap();
    let mut table = EvmTable::default();
    let mut counters = Counters::default();
    let policies = EnginePolicies::default();
    let mut cooperation = Cooperation::new(Extension::new(Observe));
    let (execution, record) = table.execute_with(
        0,
        &world,
        &entry,
        input,
        &config,
        domain,
        &policies,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
        &mut runtime,
        &mut caller,
        &mut cooperation,
    );
    assert!(record.closed, "{:?}", execution.frontiers);
    assert_indices(&record);
    let cfg = &record.regional_cfg;
    assert!(
        cfg.nodes
            .iter()
            .any(|node| node.transfer.as_ref().is_some_and(|receipt| {
                receipt.complete && receipt.input_revision == node.input_revision
            }))
    );
    assert!(cfg.nodes.iter().all(|node| {
        node.transfer
            .as_ref()
            .is_none_or(|receipt| !receipt.current)
    }));
    assert!(cfg.calls.iter().any(|call| !call.deliveries.is_empty()));
    assert!(
        cfg.calls
            .iter()
            .all(|call| call.deliveries.iter().all(|receipt| !receipt.current))
    );
}
