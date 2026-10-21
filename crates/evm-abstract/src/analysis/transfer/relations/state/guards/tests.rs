//! Direct branch facts refine live and rollback wrappers without native queries.

use crate::domain::ValueAlgebra;
use crate::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason, MachinePayload,
        machine::{ChildFrame, Continuation},
        transfer::initial,
    },
    domain::{AbstractValue, Domain, relational::CheckResult, symbolic::ExprId},
    resource::WorkBudget,
    world::{AbstractLog, Account, ByteArray, Entry, LogKey, LogSourceKind, Snapshot, World},
};
use alloy_primitives::{Address, B256, U256};

#[test]
fn direct_guards_refine_live_bytes_and_saved_storage_together() {
    let domain = Domain::default();
    let config = ExecutionConfig::default();
    let address = Address::repeat_byte(0x11);
    let mut world = World::offline(Fork::Osaka, "local guards", "known initial storage");
    world.insert(address, Account::empty()).unwrap();
    let mut payload = initial(&world, &Entry::new(address), ValueAlgebra::new(&domain)).unwrap();
    let guard = ExprId::fresh().unwrap();
    let bytes = ByteArray::exact(&[1])
        .join_guarded(
            &ByteArray::exact(&[3]),
            domain,
            guard.clone(),
            config.analysis.relations.expressions(),
        )
        .unwrap();
    payload.active_mut().memory = bytes.clone();
    payload.active_mut().returndata = bytes;
    let before = payload.store.clone();
    let mut changed = before.clone();
    changed
        .write_checked(
            address,
            &AbstractValue::constant(U256::ZERO),
            &AbstractValue::constant(U256::from(7)),
            domain,
        )
        .unwrap();
    let guarded = changed
        .join_guarded(
            &before,
            domain,
            guard.clone(),
            config.analysis.relations.expressions(),
        )
        .unwrap();
    payload.store = guarded.clone();
    payload.active_mut().saved_store = Snapshot::from_state(guarded);
    payload
        .relations
        .assume(&guard, true, &config.analysis.relations);
    let relations = payload.relations.clone();
    let mut budget = WorkBudget::new(10_000_000);
    let mut context = crate::analysis::transfer::testing::context(
        &config,
        &mut budget,
        ValueAlgebra::new(&domain),
    );
    let mut diagnostics = Vec::new();
    assert_eq!(
        super::refine(
            &mut payload,
            &relations,
            &mut context,
            &mut diagnostics,
            0,
            (
                ValueAlgebra::new(&domain),
                config.analysis.relations.expressions()
            )
        ),
        Ok(true)
    );
    assert_eq!(
        payload.active().memory.byte_at(0, domain).singleton(),
        Some(U256::ONE)
    );
    assert_eq!(
        payload.active().returndata.byte_at(0, domain).singleton(),
        Some(U256::ONE)
    );
    assert!(diagnostics.is_empty());
    assert_eq!(
        payload
            .store
            .read(address, &AbstractValue::constant(U256::ZERO), domain)
            .singleton(),
        Some(U256::from(7))
    );
    assert_eq!(
        payload
            .active()
            .saved_store
            .state()
            .read(address, &AbstractValue::constant(U256::ZERO), domain)
            .singleton(),
        Some(U256::from(7))
    );
}

#[test]
fn absent_guard_proof_retains_wrapper_and_work_refusal_stops_before_rewrite() {
    let domain = Domain::default();
    let config = ExecutionConfig::default();
    let address = Address::repeat_byte(0x22);
    let mut world = World::offline(Fork::Osaka, "local guards", "unknown condition");
    world.insert(address, Account::empty()).unwrap();
    let mut payload = initial(&world, &Entry::new(address), ValueAlgebra::new(&domain)).unwrap();
    let guard = ExprId::fresh().unwrap();
    payload.active_mut().memory = ByteArray::exact(&[1])
        .join_guarded(
            &ByteArray::exact(&[3]),
            domain,
            guard,
            config.analysis.relations.expressions(),
        )
        .unwrap();
    let before = payload.clone();
    let relations = payload.relations.clone();
    let mut diagnostics = Vec::new();
    let mut budget = WorkBudget::new(10_000_000);
    let mut context = crate::analysis::transfer::testing::context(
        &config,
        &mut budget,
        ValueAlgebra::new(&domain),
    );
    assert_eq!(
        super::refine(
            &mut payload,
            &relations,
            &mut context,
            &mut diagnostics,
            0,
            (
                ValueAlgebra::new(&domain),
                config.analysis.relations.expressions()
            )
        ),
        Ok(true)
    );
    assert_eq!(payload, before);
    assert!(diagnostics.is_empty());
    let mut budget = WorkBudget::new(0);
    context.budget = &mut budget;
    assert_eq!(
        super::refine(
            &mut payload,
            &relations,
            &mut context,
            &mut diagnostics,
            0,
            (
                ValueAlgebra::new(&domain),
                config.analysis.relations.expressions()
            )
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(payload, before);
}

#[derive(Clone, Copy, Debug)]
enum DirectCase {
    Zero,
    Nonzero,
    Undecided,
}
fn guard_config() -> ExecutionConfig {
    ExecutionConfig::default()
}
fn v(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}
fn bytes(condition: &ExprId) -> ByteArray {
    ByteArray::exact(&[1])
        .join_guarded(
            &ByteArray::exact(&[3]),
            Domain::default(),
            condition.clone(),
            guard_config().analysis.relations.expressions(),
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
    fn new(case: DirectCase) -> Self {
        let domain = Domain::default();
        let limits = guard_config().analysis.relations.expressions();
        let owners = [Address::repeat_byte(0x11), Address::repeat_byte(0x22)];
        let mut world = World::offline(Fork::Osaka, "joint guards", "frozen initial cells");
        world.insert(owners[0], Account::empty()).unwrap();
        let mut account = Account::unknown();
        account.storage.insert(U256::from(7), v(11));
        account.storage.insert(U256::from(9), v(13));
        world.insert(owners[1], account).unwrap();
        let mut before = crate::analysis::transfer::initial(
            &world,
            &Entry::new(owners[0]),
            ValueAlgebra::new(&domain),
        )
        .unwrap();
        let guards = (0..6).map(|_| ExprId::fresh().unwrap()).collect::<Vec<_>>();
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
            code_hash: B256::ZERO,
            source_kind: LogSourceKind::Runtime,
            pc: 7,
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
        let truth = match case {
            DirectCase::Zero => Some(false),
            DirectCase::Nonzero => Some(true),
            DirectCase::Undecided => None,
        };
        if let Some(truth) = truth {
            for guard in &guards {
                assert_eq!(
                    before
                        .relations
                        .assume(guard, truth, &guard_config().analysis.relations),
                    CheckResult::MayBeFeasible
                );
            }
        }
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

    fn assert_projection(&self, payload: &MachinePayload, case: DirectCase) {
        assert_eq!(self.observations(payload).len(), 14);
        assert_eq!(self.guards.len(), 6);
        let expected: Option<[u64; 14]> = match case {
            DirectCase::Zero => Some([11, 0, 0, 0, 3, 3, 3, 3, 3, 13, 3, 3, 3, 13]),
            DirectCase::Nonzero => Some([42, 19, 10, 9, 1, 1, 1, 1, 1, 57, 1, 1, 1, 57]),
            DirectCase::Undecided => {
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

/// The 14 observations are six world/log fields plus four fields in each frame.
/// Known Fresh guards select exact numeric views; unknown guards preserve both
/// alternatives, all immutable roots, and saved rollback observations.
#[test]
fn direct_guard_refinement_covers_fourteen_live_and_rollback_observations() {
    let config = guard_config();
    let domain = Domain::default();
    for case in [DirectCase::Zero, DirectCase::Nonzero, DirectCase::Undecided] {
        let fixture = Fixture::new(case);
        let mut payload = fixture.before.clone();
        let relations = payload.relations.clone();
        let mut budget = WorkBudget::new(config.max_work);
        let mut diagnostics = Vec::new();
        let mut context = crate::analysis::transfer::testing::context(
            &config,
            &mut budget,
            ValueAlgebra::new(&domain),
        );
        assert_eq!(
            super::refine(
                &mut payload,
                &relations,
                &mut context,
                &mut diagnostics,
                7,
                (
                    ValueAlgebra::new(&domain),
                    config.analysis.relations.expressions()
                )
            ),
            Ok(true),
            "{case:?}"
        );
        assert!(diagnostics.is_empty(), "{case:?}: {diagnostics:?}");
        assert!(!budget.exhausted(), "{case:?}");
        fixture.assert_projection(&payload, case);
    }
}
