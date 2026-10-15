//! Filter the two cold tests separately for the supervisor's native observer.
//! Fixture construction and pending assertions perform no native checks.

use super::{
    AbstractValue, DiagnosticKind, Domain, ExprId, FrontierReason, MAX_GUARDS, MachinePayload,
    PreparationKind, PreparedRefinement, Proofs, QueryReason, RelationState, Snapshot,
    TransferContext, U256, apply, boolean, gather, opcode, plan, prepare_refine_mode,
};
use crate::{
    Fork,
    analysis::{ChildFrame, Continuation, ExecutionConfig},
    domain::{
        relational::RelationLimits,
        symbolic::{SlotStateId, SourceId, StateBinding},
    },
    resource::WorkBudget,
    world::{AbstractLog, Account, ByteArray, Entry, LogKey, World},
};
use alloy_primitives::Address;
use embedded_smt::{Cancellation, with_cancellation};

const PC: usize = 7;

#[derive(Clone, Copy)]
enum Case {
    Zero,
    Nonzero,
    Undecided,
}

fn config() -> ExecutionConfig {
    let mut config = ExecutionConfig {
        max_work: 100_000_000,
        ..ExecutionConfig::default()
    };
    config.analysis.relations.max_nodes = 100_000;
    config.analysis.relations.max_depth = 256;
    config.analysis.relations.rlimit = 1_000_000;
    config
}

fn v(word: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(word))
}

fn op(code: u8, args: &[ExprId]) -> ExprId {
    ExprId::operation(code, args, config().analysis.relations.expressions()).unwrap()
}

fn context<'a>(config: &'a ExecutionConfig, budget: &'a mut WorkBudget) -> TransferContext<'a> {
    TransferContext {
        config,
        domain: Domain::default(),
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure: false,
        discover_code: false,
    }
}

fn condition(case: Case, relations: &mut RelationState) -> ExprId {
    let limits = config().analysis.relations;
    let write_key = ExprId::fresh().unwrap();
    let read_key = ExprId::fresh().unwrap();
    let difference = op(opcode::XOR, &[write_key.clone(), read_key.clone()]);
    let alias = match case {
        Case::Undecided => op(opcode::AND, &[difference, ExprId::constant(U256::ONE)]),
        _ => op(
            opcode::OR,
            &[
                difference.clone(),
                op(opcode::SHL, &[ExprId::constant(U256::ONE), difference]),
            ],
        ),
    };
    let alias = op(opcode::ISZERO, &[alias]);
    // Retain the frozen premise without a setup feasibility check. The OR/SHL
    // identity couples keys without giving the local scalar pass their alias.
    relations
        .observe(
            &alias,
            v(u64::from(!matches!(case, Case::Zero))).numeric(),
            &limits,
        )
        .unwrap();
    let stored = op(
        opcode::OR,
        &[
            op(
                opcode::AND,
                &[ExprId::fresh().unwrap(), ExprId::constant(U256::ONE)],
            ),
            ExprId::constant(U256::from(2)),
        ],
    );
    let state = SlotStateId::zero()
        .store(write_key, stored, limits.expressions().into())
        .unwrap();
    let read = ExprId::slot_read(state, read_key, limits.expressions()).unwrap();
    // The target also depends on an observed source binding. Dropping either
    // these definitions or the key premise changes its Boolean answer.
    let observed = SlotStateId::observed(
        SourceId::fresh().unwrap(),
        vec![StateBinding {
            index: U256::ZERO,
            value: read,
            numeric: AbstractValue::top().numeric().clone(),
        }],
        limits.expressions().into(),
    )
    .unwrap();
    ExprId::slot_read(observed, ExprId::constant(U256::ZERO), limits.expressions()).unwrap()
}

fn bytes(condition: &ExprId) -> ByteArray {
    ByteArray::exact(&[1])
        .join_guarded(
            &ByteArray::exact(&[3]),
            Domain::default(),
            condition.clone(),
            config().analysis.relations.expressions(),
        )
        .unwrap()
}

struct Fixture {
    world: World,
    before: MachinePayload,
    guards: Vec<ExprId>,
    owners: [Address; 2],
    site: LogKey,
}

impl Fixture {
    fn new(case: Case) -> Self {
        let domain = Domain::default();
        let limits = config().analysis.relations.expressions();
        let owners = [Address::repeat_byte(0x11), Address::repeat_byte(0x22)];
        let mut world = World::offline(Fork::Osaka, "joint guards", "frozen initial cells");
        world.insert(owners[0], Account::empty()).unwrap();
        let mut account = Account::unknown();
        account.storage.insert(U256::from(7), v(11));
        account.storage.insert(U256::from(9), v(13));
        world.insert(owners[1], account).unwrap();
        let mut before =
            crate::analysis::transfer::initial(&world, &Entry::new(owners[0]), domain).unwrap();
        let guards = (0..6)
            .map(|_| condition(case, &mut before.relations))
            .collect::<Vec<_>>();
        let initial = before.store.clone();
        let mut changed = initial.clone();
        changed
            .write_checked(owners[1], &v(7), &v(42), domain)
            .unwrap();
        before.store = changed
            .join_guarded(&initial, domain, guards[0].clone(), limits)
            .unwrap();
        let mut changed = before.store.clone();
        changed
            .write_transient_checked(owners[1], &v(8), &v(19), domain)
            .unwrap();
        changed.write_balance(owners[0], v(10));
        changed.write_nonce(owners[0], v(9));
        before.store = changed
            .join_guarded(&before.store, domain, guards[1].clone(), limits)
            .unwrap();
        let site = LogKey {
            address: owners[1],
            code_address: owners[0],
            pc: PC,
        };
        before
            .store
            .emit_log(
                site.clone(),
                AbstractLog {
                    topics: vec![
                        v(1).join_guarded(&v(3), domain, guards[4].clone(), limits)
                            .unwrap(),
                    ],
                    data: bytes(&guards[2]),
                },
                domain,
            )
            .unwrap();
        let mut saved = initial.clone();
        saved
            .write_checked(owners[1], &v(9), &v(57), domain)
            .unwrap();
        saved = saved
            .join_guarded(&initial, domain, guards[3].clone(), limits)
            .unwrap();
        let frame = before.active_mut();
        frame.memory = bytes(&guards[4]);
        frame.calldata = bytes(&guards[0]);
        frame.returndata = bytes(&guards[2]);
        frame.saved_store = Snapshot::from_state(saved);
        let mut child = frame.clone();
        child.key.address = owners[1];
        child.memory = bytes(&guards[5]);
        child.calldata = bytes(&guards[1]);
        before.call_stack.push_child(ChildFrame {
            state: child,
            continuation: Continuation {
                return_block: None,
                output_offset: v(0),
                output_size: v(0),
                creation: None,
            },
        });
        Self {
            world,
            before,
            guards,
            owners,
            site,
        }
    }

    fn observations(&self, payload: &MachinePayload) -> Vec<AbstractValue> {
        let domain = Domain::default();
        let log = &payload.store.possible_logs()[&self.site];
        let mut values = vec![
            payload.store.read(self.owners[1], &v(7), domain),
            payload.store.read_transient(self.owners[1], &v(8), domain),
            payload.store.read_balance(self.owners[0]),
            payload.store.nonce(self.owners[0]),
            log.topics[0].clone(),
            log.data.byte_at(0, domain),
        ];
        for frame in payload.call_stack.iter() {
            values.extend(
                [&frame.memory, &frame.calldata, &frame.returndata].map(|b| b.byte_at(0, domain)),
            );
            values.push(
                frame
                    .saved_store
                    .state()
                    .read(self.owners[1], &v(9), domain),
            );
        }
        values
    }

    fn assert_projection(&self, payload: &MachinePayload, case: Case) {
        let expected: Option<[u64; 14]> = match case {
            Case::Zero => Some([11, 0, 0, 0, 3, 3, 3, 3, 3, 13, 3, 3, 3, 13]),
            Case::Nonzero => Some([42, 19, 10, 9, 1, 1, 1, 1, 1, 57, 1, 1, 1, 57]),
            Case::Undecided => {
                assert_eq!(self.observations(payload), self.observations(&self.before));
                None
            }
        };
        if let Some(expected) = expected {
            for (value, expected) in self.observations(payload).iter().zip(expected) {
                assert_eq!(value.singleton(), Some(U256::from(expected)));
            }
        }
        for (new, old) in payload.call_stack.iter().zip(self.before.call_stack.iter()) {
            for (new, old) in [&new.memory, &new.calldata, &new.returndata]
                .into_iter()
                .zip([&old.memory, &old.calldata, &old.returndata])
            {
                assert_eq!(new.state_root(), old.state_root());
                assert_eq!(new.fresh_leaves(), old.fresh_leaves());
            }
            for address in self.owners {
                for transient in [false, true] {
                    assert_eq!(
                        new.saved_store.state().slot_root(address, transient),
                        old.saved_store.state().slot_root(address, transient)
                    );
                    assert_eq!(
                        payload.store.slot_root(address, transient),
                        self.before.store.slot_root(address, transient)
                    );
                }
            }
        }
        assert_eq!(payload.relations, self.before.relations);
        assert_eq!(
            payload
                .store
                .missing_initial_slots(&self.world, self.owners[1], &v(99)),
            self.before
                .store
                .missing_initial_slots(&self.world, self.owners[1], &v(99))
        );
        assert_eq!(
            self.world.account(self.owners[1]).unwrap().storage[&U256::from(7)],
            v(11)
        );
        let mut rollback = payload.clone();
        rollback.store.restore(payload.active().saved_store.clone());
        assert_eq!(&rollback.store, payload.active().saved_store.state());
    }
}

fn project(
    payload: &mut MachinePayload,
    config: &ExecutionConfig,
    budget: &mut WorkBudget,
    joint: bool,
) -> (Result<bool, FrontierReason>, Vec<(usize, DiagnosticKind)>) {
    let relations = payload.relations.clone();
    let mut diagnostics = Vec::new();
    let mut context = context(config, budget);
    let result = if joint {
        super::super::refine(payload, &relations, &mut context, &mut diagnostics, PC)
    } else {
        // This fixture has no refinable Fresh length or metadata operand. The
        // exact legacy guard callback is the serial reference after that phase.
        apply(
            payload,
            &relations,
            &mut context,
            &mut diagnostics,
            PC,
            (Domain::default(), config.analysis.relations.expressions()),
            Proofs::default(),
        )
    };
    (result, diagnostics)
}

fn assert_pending(fixture: &Fixture, config: &ExecutionConfig) {
    let mut budget = WorkBudget::new(config.max_work);
    let mut context = context(config, &mut budget);
    for guard in &fixture.guards {
        let mut value = boolean(guard, config.analysis.relations.expressions()).unwrap();
        assert!(matches!(
            prepare_refine_mode(
                &mut value,
                &fixture.before.relations,
                &mut context,
                PreparationKind::IntrinsicBoolean
            ),
            Ok(PreparedRefinement::Pending(_))
        ));
    }
}

#[test]
fn actual_state_joint_cold_guards() {
    let fixture = Fixture::new(Case::Nonzero);
    let config = config();
    assert_pending(&fixture, &config);
    let historical = fixture.observations(&fixture.before);
    let mut payload = fixture.before.clone();
    let mut budget = WorkBudget::new(config.max_work);
    assert_eq!(
        project(&mut payload, &config, &mut budget, true),
        (Ok(true), Vec::new())
    );
    fixture.assert_projection(&payload, Case::Nonzero);
    assert_eq!(historical, fixture.observations(&fixture.before));
    assert_eq!(
        budget.smt_cache_stats().entries,
        0,
        "common F must not enter an Fi cache"
    );
    for (_, fact) in payload
        .active()
        .memory
        .symbolic_facts()
        .filter(|(e, _)| fixture.guards.contains(e))
    {
        assert!(!fact.may_be_zero());
        assert!(
            fact.singleton().is_none(),
            "nonzero does not mean selector == 1"
        );
        assert!(fact.contains(U256::from(2)) && fact.contains(U256::from(3)));
    }
    assert!(!budget.exhausted());
}

#[test]
fn actual_state_serial_cold_guards() {
    let fixture = Fixture::new(Case::Nonzero);
    let config = config();
    assert_pending(&fixture, &config);
    let mut payload = fixture.before.clone();
    let mut budget = WorkBudget::new(config.max_work);
    assert_eq!(
        project(&mut payload, &config, &mut budget, false),
        (Ok(true), Vec::new())
    );
    fixture.assert_projection(&payload, Case::Nonzero);
    assert!(!budget.exhausted());
}

#[test]
fn serial_equivalence_overflow_encoding_unknown_and_hard_failures() {
    let config = config();
    // A proved contradiction must win before a later optional plan can spend
    // work. A false guard alone is feasible and is covered separately below.
    let mut bottom = RelationState::default();
    assert_eq!(
        bottom.assume(
            &ExprId::constant(U256::ZERO),
            true,
            &config.analysis.relations
        ),
        crate::domain::relational::CheckResult::Unsat
    );
    let mut first = Proofs::default();
    let mut first_budget = WorkBudget::new(config.max_work);
    plan(
        vec![ExprId::constant(U256::ZERO)],
        &bottom,
        &mut context(&config, &mut first_budget),
        &mut first,
    )
    .unwrap();
    assert!(first.infeasible);
    let mut stopped = Proofs::default();
    let mut exact_work = WorkBudget::new(first_budget.used());
    plan(
        vec![ExprId::constant(U256::ZERO), ExprId::fresh().unwrap()],
        &bottom,
        &mut context(&config, &mut exact_work),
        &mut stopped,
    )
    .unwrap();
    assert!(stopped.infeasible);
    assert_eq!(stopped.known.len(), 1);
    assert!(!exact_work.exhausted());
    for case in [Case::Zero, Case::Nonzero, Case::Undecided] {
        let fixture = Fixture::new(case);
        let mut joint = fixture.before.clone();
        let mut serial = fixture.before.clone();
        assert_eq!(
            project(
                &mut joint,
                &config,
                &mut WorkBudget::new(config.max_work),
                true
            ),
            (Ok(true), Vec::new())
        );
        assert_eq!(
            project(
                &mut serial,
                &config,
                &mut WorkBudget::new(config.max_work),
                false
            ),
            (Ok(true), Vec::new())
        );
        assert_eq!(joint, serial);
        fixture.assert_projection(&joint, case);
    }

    let fixture = Fixture::new(Case::Nonzero);
    let mut warmed = fixture.before.clone();
    let mut warm_budget = WorkBudget::new(config.max_work);
    assert_eq!(
        project(&mut warmed, &config, &mut warm_budget, false),
        (Ok(true), Vec::new())
    );
    let hits = warm_budget.smt_cache_stats().unique_hits;
    let mut warm_context = context(&config, &mut warm_budget);
    let conditions = gather(&fixture.before, &mut warm_context).unwrap();
    let preparation_floor = conditions.iter().fold(0usize, |cost, guard| {
        cost.saturating_add(fixture.before.relations.work_size())
            .saturating_add(
                boolean(guard, config.analysis.relations.expressions())
                    .unwrap()
                    .expression()
                    .unwrap()
                    .work_size(),
            )
    });
    let paid = warm_context.budget.used();
    let mut warm_proofs = Proofs::default();
    plan(
        conditions,
        &fixture.before.relations,
        &mut warm_context,
        &mut warm_proofs,
    )
    .unwrap();
    assert_eq!(warm_proofs.known.len(), fixture.guards.len());
    assert!(!warm_proofs.infeasible);
    assert_eq!(
        warm_context.budget.smt_cache_stats().unique_hits - hits,
        fixture.guards.len() as u64
    );
    assert!(warm_context.budget.used() - paid >= preparation_floor);

    let mut overflow = fixture.before.clone();
    // Isolate a single wrapper with a bounded plan and a serial tail. Construct
    // a linear chain so the retained roots do not grow exponentially.
    overflow.store = crate::world::Store::new(&fixture.world);
    for frame in overflow.call_stack.iter_mut() {
        frame.memory = ByteArray::memory();
        frame.calldata = ByteArray::empty();
        frame.returndata = ByteArray::empty();
        frame.saved_store = overflow.store.snapshot();
    }
    overflow.relations = RelationState::default();
    let guards = (0..=MAX_GUARDS)
        .map(|_| condition(Case::Nonzero, &mut overflow.relations))
        .collect::<Vec<_>>();
    let mut root = ByteArray::exact(&[3]);
    for guard in &guards {
        root = ByteArray::exact(&[1])
            .join_guarded(
                &root,
                Domain::default(),
                guard.clone(),
                config.analysis.relations.expressions(),
            )
            .unwrap();
    }
    overflow.active_mut().memory = root;
    let before = overflow.clone();
    let relations = overflow.relations.clone();
    let mut budget = WorkBudget::new(config.max_work);
    let mut context = context(&config, &mut budget);
    let planned = gather(&overflow, &mut context).unwrap();
    assert_eq!(planned.len(), MAX_GUARDS);
    assert_eq!(
        overflow, before,
        "collection must freeze rather than project roots"
    );
    let gathered_work = context.budget.used();
    let mut longer_tail = overflow.clone();
    longer_tail.active_mut().calldata = ByteArray::exact(&[9; 4096]);
    let mut tail_budget = WorkBudget::new(config.max_work);
    assert_eq!(
        gather(&longer_tail, &mut self::context(&config, &mut tail_budget)).unwrap(),
        planned
    );
    assert_eq!(
        tail_budget.used(),
        gathered_work,
        "a full plan must not traverse or prepay later wrappers"
    );
    let mut proofs = Proofs::default();
    plan(planned, &relations, &mut context, &mut proofs).unwrap();
    assert_eq!(proofs.known.len(), MAX_GUARDS);
    let mut interrupted = overflow.clone();
    assert_eq!(
        apply(
            &mut interrupted,
            &relations,
            &mut self::context(&config, &mut WorkBudget::new(0)),
            &mut Vec::new(),
            PC,
            (Domain::default(), config.analysis.relations.expressions()),
            proofs
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(
        interrupted.active().memory,
        before.active().memory,
        "a failed serial tail must not publish a partial wrapper"
    );
    assert_eq!(
        project(
            &mut overflow,
            &config,
            &mut WorkBudget::new(config.max_work),
            true
        ),
        (Ok(true), Vec::new())
    );
    for guard in &guards {
        let (_, fact) = overflow
            .active()
            .memory
            .symbolic_facts()
            .find(|(e, _)| *e == guard)
            .expect("the unplanned tail must also be proved");
        assert!(!fact.may_be_zero());
        assert!(fact.singleton().is_none());
    }

    // Put independent targets in separate wrappers, so each wrapper and Fi can
    // fit while their common encoding exceeds the same max_nodes policy.
    let mut flat = fixture.before.clone();
    flat.store = before.store.clone();
    for frame in flat.call_stack.iter_mut() {
        frame.saved_store = flat.store.snapshot();
    }
    flat.active_mut().returndata = bytes(&fixture.guards[3]);
    let expressions = fixture
        .guards
        .iter()
        .map(|g| {
            boolean(g, config.analysis.relations.expressions())
                .unwrap()
                .expression()
                .unwrap()
                .clone()
        })
        .collect::<Vec<_>>();
    let bound = (1..config.analysis.relations.max_nodes)
        .find(|max_nodes| {
            let limits = RelationLimits {
                max_nodes: *max_nodes,
                ..config.analysis.relations
            };
            let mut budget = WorkBudget::new(config.max_work);
            expressions.iter().all(|e| {
                assert!(budget.charge(flat.relations.work_size().saturating_add(e.work_size())));
                matches!(
                    flat.relations.probe_unique_value_cached(
                        e,
                        &limits,
                        &mut budget
                            .smt_cache_query(limits.cache_scope())
                            .with_allowance(0)
                    ),
                    Ok(None)
                )
            }) && flat.call_stack.iter().all(|frame| {
                [&frame.memory, &frame.calldata, &frame.returndata]
                    .iter()
                    .all(|b| b.guard_conditions(limits.expressions()).is_ok())
            })
        })
        .expect("individual encodings and wrappers fit a finite limit");
    let mut limited = config.clone();
    limited.analysis.relations.max_nodes = bound;
    let cost = expressions
        .iter()
        .fold(flat.relations.work_size(), |n, e| {
            n.saturating_add(e.work_size())
        })
        .saturating_add(limited.analysis.relations.rlimit as usize);
    assert_eq!(
        flat.relations.unique_many_budgeted(
            &expressions,
            &limited.analysis.relations,
            &mut WorkBudget::new(config.max_work).query_budget(cost)
        ),
        Err(QueryReason::ExpressionLimit)
    );
    let mut serial = flat.clone();
    assert_eq!(
        project(
            &mut flat,
            &limited,
            &mut WorkBudget::new(config.max_work),
            true
        ),
        (Ok(true), Vec::new())
    );
    assert_eq!(
        project(
            &mut serial,
            &limited,
            &mut WorkBudget::new(config.max_work),
            false
        ),
        (Ok(true), Vec::new())
    );
    assert_eq!(
        flat, serial,
        "joint encoding refusal must retain serial coverage"
    );

    let mut unknown_config = config.clone();
    unknown_config.analysis.relations.rlimit = 1;
    let mut unknown = before.clone();
    let (result, diagnostics) = project(
        &mut unknown,
        &unknown_config,
        &mut WorkBudget::new(config.max_work),
        true,
    );
    assert_eq!(result, Ok(true));
    assert!(diagnostics.iter().any(|(_, reason)| matches!(
        reason,
        DiagnosticKind::RelationalPrecisionLost(QueryReason::ResourceLimit)
    )));
    assert_eq!(
        unknown.active().memory,
        before.active().memory,
        "Unknown must not become a guard proof"
    );

    let token = Cancellation::new();
    token.cancel();
    let mut outside = WorkBudget::new(config.max_work);
    with_cancellation(&token, || {
        let mut payload = before.clone();
        assert_eq!(
            project(&mut payload, &config, &mut outside, true).0,
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
        assert_eq!(payload.active().memory, before.active().memory);
    });
    let mut captured = with_cancellation(&token, || WorkBudget::new(config.max_work));
    for budget in [&mut captured, &mut WorkBudget::new(0)] {
        let mut payload = before.clone();
        assert_eq!(
            project(&mut payload, &config, budget, true).0,
            Err(FrontierReason::Work)
        );
        assert_eq!(payload, before);
        assert!(budget.exhausted());
    }
}
