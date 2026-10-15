//! Exercise the real branch path with one analysis-owned cache and work ledger.

use super::{
    AbstractValue, Domain, Execution, ExecutionConfig, ExprId, FrontierReason, QueryReason,
    TransferContext, U256, WorkBudget, execution, opcode,
};
use crate::{
    analysis::{DiagnosticKind, MachinePayload},
    domain::{
        relational::{CheckResult, Constraint},
        symbolic::{ByteStateId, ByteView, SourceId},
    },
};
use embedded_smt::{Cancellation, with_cancellation};

fn config() -> ExecutionConfig {
    let mut config = ExecutionConfig::default();
    config.analysis.relations.rlimit = 1_000_000;
    config
}

fn result() -> Execution {
    let mut result = execution();
    // Keep call-value projection concrete to isolate the paid branch query.
    result.payload.active_mut().call_value = AbstractValue::constant(U256::ZERO);
    result
}

fn run(
    result: &mut Execution,
    condition: &AbstractValue,
    nonzero: bool,
    config: &ExecutionConfig,
    budget: &mut WorkBudget,
) -> Option<MachinePayload> {
    let mut context = TransferContext {
        config,
        domain: Domain::default(),
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    };
    super::super::branch(result, condition, nonzero, &mut context, 7)
}

fn query_fee(result: &Execution, condition: &AbstractValue, config: &ExecutionConfig) -> usize {
    result
        .payload
        .relations
        .work_size()
        .saturating_add(condition.expression().unwrap().work_size())
        .saturating_add(config.analysis.relations.rlimit as usize)
}

fn assert_guard(payload: &MachinePayload, condition: &AbstractValue, nonzero: bool) {
    assert!(payload.relations.constraints().any(|atom| atom
        == &Constraint::Truth {
            expression: condition.expression().unwrap().clone(),
            nonzero,
        }));
}

#[test]
fn warm_actual_branches_pay_the_same_mandatory_fee_and_share_the_workbudget_owner() {
    let config = config();
    let mut budget = WorkBudget::new(100_000_000);
    for nonzero in [false, true] {
        let mut first_mandatory = None;
        for repeat in 0..2 {
            let mut result = result();
            let before_payload = result.payload.clone();
            let condition = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
            let fee = query_fee(&result, &condition, &config);
            let before_work = budget.used();
            let before_cache = budget.smt_cache_stats();
            let successor = run(&mut result, &condition, nonzero, &config, &mut budget).unwrap();
            let maintenance = (budget.smt_cache_stats().maintenance_work
                - before_cache.maintenance_work) as usize;
            let mandatory = budget.used() - before_work - maintenance;
            // The fixture also retains an unrelated symbolic calldata length.
            // State refinement pays its relevance check before skipping it.
            let length = successor.active().calldata.len();
            assert!(length.singleton().is_none());
            let length_work = successor
                .relations
                .work_size()
                .saturating_add(length.expression().unwrap().work_size());
            // The guard planner prepays an extra read-only traversal for each
            // state wrapper. This fixture has no ITE, so there is no additional
            // guard retention or native query fee. Count the wrappers rather
            // than payload.work_size(), which also includes scalar/frame data.
            let limits = config.analysis.relations.expressions();
            assert!(successor.store.guard_conditions(limits).unwrap().is_empty());
            let mut byte_work = 0;
            let mut checkpoint_work = 0;
            for frame in successor.call_stack.iter() {
                for bytes in [&frame.memory, &frame.calldata, &frame.returndata] {
                    assert!(bytes.guard_conditions(limits).unwrap().is_empty());
                    byte_work += bytes.work_size();
                }
                assert!(
                    frame
                        .saved_store
                        .state()
                        .guard_conditions(limits)
                        .unwrap()
                        .is_empty()
                );
                checkpoint_work += frame.saved_store.state().work_size();
            }
            let store_work = successor.store.work_size();
            let collection_work = (store_work + byte_work + checkpoint_work) * 2;
            eprintln!(
                "branch mandatory fee: nonzero={nonzero} repeat={repeat} store={store_work} bytes={byte_work} checkpoint={checkpoint_work} collection={collection_work} mandatory={mandatory}"
            );
            assert_eq!(
                mandatory,
                before_payload.work_size()
                    + fee
                    + successor.work_size() * 2
                    + length_work
                    + collection_work
            );
            assert!(maintenance <= fee / 4);
            if let Some(first) = first_mandatory {
                assert_eq!(
                    mandatory, first,
                    "a warm branch still pays the full query fee once"
                );
            } else {
                first_mandatory = Some(mandatory);
            }
            assert_guard(&successor, &condition, nonzero);
            assert!(!successor.relations.is_bottom());
            assert_eq!(result.payload, before_payload);
            assert!(result.frontiers.is_empty());
            assert!(result.diagnostics.is_empty());
            assert_eq!(
                budget.smt_cache_stats().candidate_hits - before_cache.candidate_hits,
                repeat
            );
            assert!(!budget.exhausted());
        }
    }
    assert_eq!(budget.smt_cache_stats().candidate_hits, 2);
    assert_eq!(budget.smt_cache_stats().entries, 2);
    // Reconstructing transfer contexts above retained the same owner; a new
    // analysis owner starts cold even for another alpha-renamed conjunction.
    let mut other = WorkBudget::new(100_000_000);
    let condition = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    assert!(run(&mut result(), &condition, true, &config, &mut other).is_some());
    assert_eq!(other.smt_cache_stats().candidate_hits, 0);
}

#[test]
fn cold_and_warm_unsat_branches_prune_after_exactly_one_original_query_fee() {
    let config = config();
    let mut budget = WorkBudget::new(100_000_000);
    let mut checks = 0;
    for nonzero in [false, true] {
        for _ in 0..2 {
            let mut result = result();
            let condition = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
            assert_eq!(
                result.payload.relations.assume_checked(
                    condition.expression().unwrap(),
                    !nonzero,
                    &config.analysis.relations,
                ),
                Ok(CheckResult::Sat)
            );
            let before_payload = result.payload.clone();
            let fee = query_fee(&result, &condition, &config);
            let before_work = budget.used();
            let before_cache = budget.smt_cache_stats();
            assert!(run(&mut result, &condition, nonzero, &config, &mut budget).is_none());
            let maintenance = (budget.smt_cache_stats().maintenance_work
                - before_cache.maintenance_work) as usize;
            assert_eq!(
                budget.used() - before_work - maintenance,
                before_payload.work_size() + fee
            );
            assert!(maintenance <= fee / 4);
            assert_eq!(
                budget.smt_cache_stats().candidate_hits - before_cache.candidate_hits,
                u64::from(checks > 0)
            );
            checks += 1;
            assert_eq!(result.payload, before_payload);
            assert!(!result.payload.relations.is_bottom());
            assert!(result.frontiers.is_empty());
            assert!(result.diagnostics.is_empty());
        }
    }
    assert_eq!(budget.smt_cache_stats().candidate_hits, 3);
    assert_eq!(budget.smt_cache_stats().entries, 1);
    assert!(!budget.exhausted());
}

#[test]
fn warm_optional_maintenance_refusal_runs_the_oracle_without_marking_work_exhausted() {
    let config = config();
    let mut result = result();
    let expression = ExprId::fresh().unwrap();
    let condition = AbstractValue::top().with_expression(expression.clone());
    assert_eq!(
        result
            .payload
            .relations
            .assume_checked(&expression, true, &config.analysis.relations),
        Ok(CheckResult::Sat)
    );
    let mut budget = WorkBudget::new(100_000_000);
    assert!(run(&mut result, &condition, false, &config, &mut budget).is_none());
    let before_payload = result.payload.clone();
    let mandatory = result.payload.work_size() + query_fee(&result, &condition, &config);
    let reserve = 100_000_000 - budget.used() - mandatory;
    assert!(budget.charge(reserve));
    let before_work = budget.used();
    let before_cache = budget.smt_cache_stats();
    assert!(run(&mut result, &condition, false, &config, &mut budget).is_none());
    assert_eq!(budget.used() - before_work, mandatory);
    assert_eq!(budget.used(), 100_000_000);
    assert_eq!(
        budget.smt_cache_stats().candidate_hits,
        before_cache.candidate_hits
    );
    assert_eq!(
        budget.smt_cache_stats().maintenance_work,
        before_cache.maintenance_work
    );
    assert_eq!(budget.smt_cache_stats().bypasses, before_cache.bypasses + 1);
    assert_eq!(result.payload, before_payload);
    assert!(result.frontiers.is_empty());
    assert!(!budget.exhausted());
}

#[test]
fn insufficient_quarter_allowance_preserves_real_sat_and_unsat_branch_answers() {
    let mut config = config();
    config.analysis.relations.rlimit = 1000;
    let input = ExprId::fresh().unwrap();
    let expression =
        ExprId::operation(opcode::EQ, &[input.clone(), input], Default::default()).unwrap();
    let condition = AbstractValue::unsigned_range(U256::ZERO, U256::ONE)
        .unwrap()
        .with_expression(expression);
    for nonzero in [false, true] {
        let mut result = result();
        let fee = query_fee(&result, &condition, &config);
        let mut budget = WorkBudget::new(100_000_000);
        let successor = run(&mut result, &condition, nonzero, &config, &mut budget);
        assert_eq!(successor.is_some(), nonzero);
        if let Some(successor) = successor {
            assert_guard(&successor, &condition, nonzero);
        }
        assert_eq!(budget.smt_cache_stats().bypasses, 1);
        assert_eq!(budget.smt_cache_stats().entries, 0);
        assert!(budget.smt_cache_stats().maintenance_work as usize <= fee / 4);
        assert!(result.frontiers.is_empty());
        assert!(result.diagnostics.is_empty());
        assert!(!budget.exhausted());
    }
}

#[test]
fn mandatory_work_refusal_precedes_rejected_guard_retention_even_with_a_warm_owner() {
    let config = config();
    let mut result = result();
    let first = ExprId::fresh().unwrap();
    let condition = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    assert_eq!(
        result
            .payload
            .relations
            .assume_checked(&first, true, &config.analysis.relations),
        Ok(CheckResult::Sat)
    );
    let mut budget = WorkBudget::new(100_000_000);
    assert!(run(&mut result, &condition, true, &config, &mut budget).is_some());
    assert_eq!(budget.smt_cache_stats().entries, 1);
    let mut restricted = config;
    restricted.analysis.relations.max_constraints = 1;
    let clone_fee = result.payload.work_size();
    let fee = query_fee(&result, &condition, &restricted);
    let reserve = 100_000_000 - budget.used() - (clone_fee + fee - 1);
    assert!(budget.charge(reserve));
    let before_work = budget.used();
    let before_cache = budget.smt_cache_stats();
    let before_payload = result.payload.clone();
    assert!(run(&mut result, &condition, true, &restricted, &mut budget).is_none());
    assert_eq!(budget.used() - before_work, clone_fee);
    assert_eq!(result.payload, before_payload);
    assert_eq!(budget.smt_cache_stats(), before_cache);
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert!(result.outcomes.is_empty());
    assert!(budget.exhausted());
}

#[test]
fn native_and_encoding_unknown_keep_both_complete_guarded_successors() {
    let expression = ExprId::operation(
        opcode::EQ,
        &[ExprId::fresh().unwrap(), ExprId::constant(U256::ONE)],
        Default::default(),
    )
    .unwrap();
    let mut native = config();
    native.analysis.relations.rlimit = 1;
    let mut encoding = config();
    encoding.analysis.relations.max_nodes = 64;
    let length = ExprId::constant(U256::from(64));
    let hash = ExprId::keccak(
        ByteView::new(
            ByteStateId::input(SourceId::fresh().unwrap(), length.clone()),
            ExprId::constant(U256::ZERO),
            length,
        ),
        encoding.analysis.relations.expressions(),
    )
    .unwrap();
    for (expression, config, reason) in [
        (expression, native, QueryReason::ResourceLimit),
        (hash, encoding, QueryReason::ExpressionLimit),
    ] {
        let condition = AbstractValue::top().with_expression(expression);
        let mut budget = WorkBudget::new(100_000_000);
        for nonzero in [false, true] {
            let mut result = result();
            let successor = run(&mut result, &condition, nonzero, &config, &mut budget).unwrap();
            assert_guard(&successor, &condition, nonzero);
            assert!(!successor.relations.is_bottom());
            assert_eq!(
                result.diagnostics,
                [(7, DiagnosticKind::RelationalPrecisionLost(reason.clone()))]
            );
            assert!(result.frontiers.is_empty());
            assert!(result.outcomes.is_empty());
            assert!(!budget.exhausted());
        }
        assert_eq!(budget.smt_cache_stats().entries, 0);
    }
}

#[test]
fn current_cancellation_and_captured_budget_token_preserve_the_existing_frontier_order() {
    let config = config();
    let condition = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    let mut budget = WorkBudget::new(100_000_000);
    assert!(run(&mut result(), &condition, true, &config, &mut budget).is_some());
    let before_cache = budget.smt_cache_stats();
    let before_work = budget.used();
    let mut result = result();
    let before_payload = result.payload.clone();
    let mandatory = result.payload.work_size() + query_fee(&result, &condition, &config);
    let token = Cancellation::new();
    token.cancel();
    with_cancellation(&token, || {
        assert!(run(&mut result, &condition, true, &config, &mut budget).is_none());
    });
    assert_eq!(budget.used() - before_work, mandatory);
    assert_eq!(budget.smt_cache_stats(), before_cache);
    assert_eq!(result.payload, before_payload);
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(
        result.frontiers[0].reason,
        FrontierReason::Relations(QueryReason::Cancelled)
    );
    assert!(!budget.exhausted());

    let mut captured = with_cancellation(&token, || WorkBudget::new(100_000_000));
    let mut result = super::execution();
    assert!(run(&mut result, &condition, true, &config, &mut captured).is_none());
    assert_eq!(captured.used(), 0);
    assert!(captured.exhausted());
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
}
