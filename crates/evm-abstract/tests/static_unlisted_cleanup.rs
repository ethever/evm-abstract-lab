//! Detailed static-call completion must cover cleanup outside the observed world.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        self, ExecutionConfig, FrameCode, MachineEdgeKind, OutcomeKind, ProcedureSummaryKind,
        Status,
    },
    domain::{
        AbstractValue, Domain,
        symbolic::{ExprId, ExprKind, SlotStateId, SlotStateKind, SourceId},
    },
    world::{Account, ByteArray, Code, Entry, Existence, Store, World},
};
use std::collections::BTreeMap;

const A: u64 = 0x101;
const U: u64 = 0x202;
const C: u64 = 0x303;
const ROOT_CODE: &str = "5f5f5f5f61020262fffffffa5000";
const UNKNOWN_CODE_WITNESS: &str = "5f5f5f5f61030362fffffffa5000";

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

/// A partial assignment extends by zero to every unassigned source cell.
/// Full source identities include owner namespaces; word identities include
/// input scopes. Retention ITEs always select nonzero, without branch search.
/// Unobserved scalar fields are independent Cartesian bounds. Storage always
/// needs a checked immutable expression or a proved singleton, never bare top.
#[derive(Default)]
struct JointWitness {
    words: BTreeMap<ExprId, U256>,
    cells: BTreeMap<(SourceId, U256), U256>,
}

impl JointWitness {
    fn assign<K: Ord>(map: &mut BTreeMap<K, U256>, key: K, value: U256) -> bool {
        match map.get(&key) {
            Some(previous) => *previous == value,
            None => {
                map.insert(key, value);
                true
            }
        }
    }

    fn expression(&mut self, expression: &ExprId, expected: U256) -> bool {
        match expression.kind() {
            ExprKind::Constant(value) => *value == expected,
            ExprKind::Input(_) | ExprKind::Fresh(_) => {
                Self::assign(&mut self.words, expression.clone(), expected)
            }
            ExprKind::SlotRead { state, key } => key
                .as_constant()
                .is_some_and(|key| self.slot(state, key, expected)),
            ExprKind::Ite {
                condition,
                then_value,
                ..
            } => self.expression(condition, U256::from(1)) && self.expression(then_value, expected),
            _ => false,
        }
    }

    fn slot(&mut self, state: &SlotStateId, key: U256, expected: U256) -> bool {
        match state.kind() {
            SlotStateKind::Zero => expected == U256::ZERO,
            SlotStateKind::Input { source, bindings } if bindings.is_empty() => {
                Self::assign(&mut self.cells, (source.clone(), key), expected)
            }
            SlotStateKind::Ite {
                condition,
                then_state,
                ..
            } => self.expression(condition, U256::from(1)) && self.slot(then_state, key, expected),
            // This fixture has no writes or observed initial cells. Refuse
            // other grammars rather than guessing their complete valuation.
            _ => false,
        }
    }

    fn scalar(&mut self, value: &AbstractValue, expected: u64) -> bool {
        let expected = U256::from(expected);
        value.contains(expected)
            && !value.symbolic_limit_reached()
            && match value.expression() {
                Some(expression) => self.expression(expression, expected),
                None => value.identity().input().is_none(),
            }
    }

    fn storage(&mut self, store: &Store, owner: Address, expected: u64) -> bool {
        let value = store
            .read_checked(owner, &word(8), Domain::default())
            .unwrap();
        (value.expression().is_some() || value.singleton().is_some())
            && self.scalar(&value, expected)
    }

    fn account(
        &mut self,
        store: &Store,
        owner: Address,
        code: &Code,
        presence: Existence,
        values: [u64; 3],
    ) -> bool {
        let [balance, nonce, slot] = values;
        (matches!(store.code(owner), None | Some(Code::Unknown)) || store.code(owner) == Some(code))
            && (store.existence(owner) == Existence::Unknown || store.existence(owner) == presence)
            && self.scalar(&store.read_balance(owner), balance)
            && self.scalar(&store.nonce(owner), nonce)
            && self.storage(store, owner, slot)
    }
}

#[test]
fn completed_static_unknown_call_covers_unlisted_cleanup_under_one_original_input() {
    let fork = Fork::Osaka;
    let [a, u, c] = [A, U, C].map(address);
    let mut world = World::new(fork, "detailed static unlisted cleanup");
    let mut root = Account::from_hex(ROOT_CODE, fork).unwrap();
    root.balance = word(10);
    root.nonce = word(1);
    world.insert(a, root).unwrap();
    assert_eq!(world.accounts().len(), 1);
    assert!(!world.accounts().contains_key(&u));
    assert!(!world.accounts().contains_key(&c));
    // This is one allowed unknown initial implementation, never a World fact.
    // It STATICCALLs C, POPs the success flag, then STOPs. C may be empty with
    // nonzero storage: storage is not part of the empty-account predicate.
    let unknown_code = Account::from_hex(UNKNOWN_CODE_WITNESS, fork).unwrap().code;
    let entry = Entry::concrete(a, address(0x900), word(0), ByteArray::empty());
    let config = ExecutionConfig {
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    };
    assert!(!config.root_terminal_coverage);
    assert_eq!(config.max_work, 20_000_000);
    let report = analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(
        report.status(),
        Status::Converged,
        "an incomplete probe is not completed-analysis evidence: {:?}",
        report.frontiers()
    );
    assert!(report.frontiers().is_empty());
    assert!(report.root_terminal_coverage().is_none());
    assert!(!report.config().root_terminal_coverage);
    assert_eq!(report.config().max_work, 20_000_000);
    let original = &report.states()[0].entry;
    assert_eq!(original.active().key.mode, FrameCode::Runtime);
    assert_eq!(original.active().key.code_address, a);
    assert!(!original.active().key.is_static);
    assert_eq!(original.active().call_value.singleton(), Some(U256::ZERO));
    assert_eq!(original.active().calldata, ByteArray::empty());
    assert!(report.states()[0].executed_pcs.contains(&11));
    assert!(
        report.states()[0]
            .execution_evidence()
            .is_some_and(|evidence| evidence.is_current())
    );
    assert!(
        report
            .procedure_summaries()
            .iter()
            .all(|summary| summary.closed)
    );
    assert!(report.procedure_summaries().iter().any(|summary| {
        summary.kind == ProcedureSummaryKind::UnknownEnvironment
            && summary.root_procedure.is_some_and(|root| {
                let procedure = &summary.procedures[root];
                procedure.frame.mode == FrameCode::Unknown
                    && procedure.frame.code_address == u
                    && procedure.frame.address == u
                    && procedure.frame.is_static
                    && procedure
                        .outputs
                        .iter()
                        .any(|output| output.kind == OutcomeKind::Return)
            })
    }));
    assert!(
        report
            .edges()
            .iter()
            .any(|edge| matches!(edge.kind, MachineEdgeKind::Summary(OutcomeKind::Return)))
    );

    let initial = &original.store;
    let saved = original.active().saved_store.state();
    let history = initial
        .read_checked(c, &word(8), Domain::default())
        .unwrap();
    assert!(history.expression().is_some());
    let root_history = initial.slot_root(a, false).unwrap();
    let root_code = &report.world().accounts()[&a].code;
    for post in [0, 7] {
        // Search the union of actual Return tuples. Separate outcomes may
        // cover deletion and retention; each uses its own complete assignment.
        let covered = report.outcomes().iter().any(|outcome| {
            if outcome.kind != OutcomeKind::Return || outcome.data != ByteArray::empty() {
                return false;
            }
            let state = &report.states()[outcome.state];
            if state.active().mode != FrameCode::Runtime
                || state.active().code_address != a
                || !state.executed_pcs.contains(&13)
                || !state
                    .execution_evidence()
                    .is_some_and(|evidence| evidence.is_current())
            {
                return false;
            }
            let mut joint = JointWitness::default();
            let post_presence = if post == 0 {
                Existence::Absent
            } else {
                Existence::Present
            };
            for (store, slot, presence) in [
                (initial, 7, Existence::Present),
                (saved, 7, Existence::Present),
                (&outcome.store, post, post_presence),
            ] {
                if !joint.account(store, a, root_code, Existence::Present, [10, 1, 0])
                    || store.code(a) != Some(root_code)
                    || store.existence(a) != Existence::Present
                    || store.read_balance(a).singleton() != Some(U256::from(10))
                    || store.nonce(a).singleton() != Some(U256::from(1))
                    || store.slot_root(a, false).unwrap() != root_history
                    || !joint.account(store, u, &unknown_code, Existence::Present, [0, 1, 0])
                    || !joint.account(store, c, &Code::Empty, presence, [0, 0, slot])
                    || !store.possible_logs().is_empty()
                    || !joint.scalar(store.balance_default(), 0)
                    || !joint.scalar(&store.nonce_default(), 0)
                    || !matches!(
                        store.code_default(),
                        None | Some(Code::Unknown | Code::Empty)
                    )
                    || !matches!(
                        store.existence_default(),
                        Existence::Unknown | Existence::Absent
                    )
                    || store.addresses().into_iter().any(|owner| {
                        ![a, u, c].contains(&owner)
                            && !joint.account(store, owner, &Code::Empty, Existence::Absent, [0; 3])
                    })
                {
                    return false;
                }
            }
            // Re-evaluate the old read after the post-state assignment. Even
            // numerical top cannot admit both values from one immutable cell.
            if !joint.scalar(&history, 7) || joint.scalar(&history, 0) {
                return false;
            }
            let store = &outcome.store;
            store.addresses().into_iter().chain([a, u, c]).all(|owner| {
                store.created_in_transaction(owner) == Some(false)
                    && store.pending_destruction(owner) == Some(false)
                    && store.touched_in_transaction(owner) == Some(false)
                    && matches!(
                        store.slot_root(owner, true).unwrap().kind(),
                        SlotStateKind::Zero
                    )
            }) && store.created_default() == Some(false)
                && store.pending_destruction_default() == Some(false)
                && store.touched_default() == Some(false)
                && matches!(store.global_transient_state().kind(), SlotStateKind::Zero)
                && store.global_transient_default().singleton() == Some(U256::ZERO)
                && store.transient_slots().is_empty()
        });
        assert!(
            covered,
            "no whole Return witness for original C[8]=7 -> C[8]={post}"
        );
    }
}
