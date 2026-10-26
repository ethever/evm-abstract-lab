use super::*;
use crate::{
    Address, Fork, U256,
    domain::ValueAlgebra,
    world::{
        EvmAccount, EvmByteArray, Existence, LogSourceKind,
        store::{
            LogKey,
            summary::{
                SummaryLog,
                test_support::{CALLS, Calls, Native, Policy},
            },
        },
    },
};

fn word(value: u64) -> EvmValue<Native> {
    EvmValue::from_numeric(Native::constant(U256::from(value)))
}

fn store() -> EvmStoreSummary<Native> {
    let plane = || SummaryPlane {
        slots: OrderedMap::new(),
        defaults: OrderedMap::new(),
        global_default: EvmValue::top(),
        initial_independence: InitialIndependence::new(false),
    };
    EvmStoreSummary {
        persistent: plane(),
        transient: plane(),
        balances: OrderedMap::new(),
        balance_default: EvmValue::top(),
        codes: OrderedMap::new(),
        nonces: OrderedMap::new(),
        existence: OrderedMap::new(),
        created: OrderedMap::new(),
        created_default: None,
        pending_destruction: OrderedMap::new(),
        pending_destruction_default: None,
        touched: OrderedMap::new(),
        touched_default: None,
        possible_logs: OrderedMap::new(),
        logs_unknown: false,
    }
}

fn covers(left: &EvmStoreSummary<Native>, right: &EvmStoreSummary<Native>) -> bool {
    left.covers_with_payment(
        right,
        ValueAlgebra::new(Policy::default()).borrowed(),
        &mut WorkBudget::new(10_000_000),
    )
    .unwrap()
}

#[test]
fn summary_coverage_sparse_defaults_and_omitted_keys_use_each_inputs_fallback() {
    let _scope = CALLS.scope();
    let owner = Address::repeat_byte(1);
    let key = (owner, U256::ONE);
    let broad = store();
    for component in 0..5 {
        let mut narrow = store();
        match component {
            0 => {
                narrow.persistent.defaults.insert(owner, word(0));
            }
            1 => {
                narrow.persistent.slots.insert(key, word(7));
            }
            2 => {
                narrow.transient.slots.insert(key, word(7));
            }
            3 => {
                narrow.balances.insert(owner, word(7));
            }
            _ => {
                narrow.nonces.insert(owner, word(7));
            }
        }
        assert!(covers(&broad, &narrow));
        assert!(
            !covers(&narrow, &broad),
            "omitted incoming key is Top: {component}"
        );
    }
    let mut left = store();
    left.persistent.global_default = word(0);
    let mut right = left.clone();
    right.persistent.slots.insert(key, word(7));
    assert!(
        !covers(&left, &right),
        "incoming extra slot overrides its default"
    );
    left.persistent.defaults.insert(owner, EvmValue::top());
    assert!(covers(&left, &right));
    left.persistent.slots.insert(key, word(0));
    assert!(
        !covers(&left, &right),
        "explicit retained slot overrides owner Top"
    );
    right.persistent.slots.clear();
    assert!(
        covers(&left, &right),
        "incoming omission resolves to its zero default"
    );
}

#[test]
fn summary_coverage_private_predicate_is_not_public_top_and_proof_removes_real_hooks() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let mut left = store();
    left.balance_default = EvmValue::from_numeric(Native::private());
    let top = store();
    assert_eq!(
        left.balance_default.numeric().observation(),
        top.balance_default.numeric().observation()
    );
    assert!(!covers(&left, &top));
    CALLS.set(Calls::default());
    let mut budget = WorkBudget::new(1_000_000);
    assert!(
        left.covers_with_payment(&left, domain.borrowed(), &mut budget)
            .unwrap()
    );
    let calls = CALLS.get();
    assert!(calls.equal > 0 && calls.is_top > 0);
    assert_eq!(
        calls.clones
            + calls.projects
            + calls.joins
            + calls.widens
            + calls.canonical
            + calls.exports
            + calls.imports
            + calls.constructions,
        0
    );
    CALLS.set(Calls::default());
    let joined = left
        .join_with_payment(
            &top,
            domain.borrowed(),
            false,
            &mut WorkBudget::new(1_000_000),
        )
        .unwrap();
    assert!(CALLS.get().joins > 0 && CALLS.get().projects > 0 && CALLS.get().canonical > 0);
    assert!(joined.balance_default.contains(U256::from(7)));
    assert!(!left.balance_default.contains(U256::from(7)));
}

#[test]
fn summary_coverage_code_existence_lifecycle_touch_and_initial_dependency_are_required() {
    let _scope = CALLS.scope();
    let owner = Address::repeat_byte(1);
    for component in 0..9 {
        let mut left = store();
        let right = store();
        match component {
            0 => {
                left.codes.insert(owner, Code::Empty);
            }
            1 => {
                left.existence.insert(owner, Existence::Present);
            }
            2 => {
                left.created.insert(owner, Some(true));
            }
            3 => {
                left.pending_destruction.insert(owner, Some(true));
            }
            4 => {
                left.touched.insert(owner, Some(true));
            }
            5 => {
                left.created_default = Some(false);
            }
            6 => {
                left.pending_destruction_default = Some(false);
            }
            7 => {
                left.touched_default = Some(false);
            }
            _ => {
                left.persistent.initial_independence.global_default = true;
            }
        }
        assert!(!covers(&left, &right), "retained metadata: {component}");
        assert!(covers(&right, &left), "weaker metadata: {component}");
    }
    let mut left = store();
    let mut right = store();
    left.transient
        .initial_independence
        .slots
        .insert((owner, U256::ONE), true);
    right
        .transient
        .initial_independence
        .defaults
        .insert(owner, true);
    assert!(covers(&left, &right));
    right
        .transient
        .initial_independence
        .slots
        .insert((owner, U256::ONE), false);
    assert!(!covers(&left, &right));
    let code = EvmAccount::<Native>::from_hex("0x600000", Fork::Osaka)
        .unwrap()
        .code;
    left = store();
    right = store();
    left.codes.insert(owner, code.clone());
    right.codes.insert(owner, code);
    assert!(covers(&left, &right));
    right.codes.insert(
        owner,
        EvmAccount::<Native>::from_hex("0x600000", Fork::Osaka)
            .unwrap()
            .code,
    );
    assert!(
        !covers(&left, &right),
        "independent capture is an unsupported proof"
    );
    left.codes.insert(owner, Code::Delegation(owner));
    right
        .codes
        .insert(owner, Code::Delegation(Address::repeat_byte(2)));
    assert!(!covers(&left, &right));
}

#[test]
fn summary_coverage_logs_keep_source_topics_payload_and_unknown_mode() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let key = LogKey {
        address: Address::repeat_byte(1),
        code_address: Address::repeat_byte(2),
        code_hash: alloy_primitives::B256::ZERO,
        source_kind: LogSourceKind::Runtime,
        pc: 7,
    };
    let log = SummaryLog {
        topics: vec![EvmValue::from_numeric(Native::private())],
        data: EvmByteArray::exact(&[1, 2])
            .summary_projection(domain.borrowed())
            .unwrap(),
    };
    let mut left = store();
    left.possible_logs.insert(key.clone(), log);
    let mut right = left.clone();
    assert!(covers(&left, &right));
    right.possible_logs.get_mut(&key).unwrap().topics[0] = EvmValue::top();
    assert!(!covers(&left, &right));
    left.possible_logs.get_mut(&key).unwrap().topics[0] = EvmValue::top();
    assert!(covers(&left, &right));
    right
        .possible_logs
        .get_mut(&key)
        .unwrap()
        .topics
        .push(word(1));
    assert!(!covers(&left, &right));
    right = left.clone();
    right.possible_logs.get_mut(&key).unwrap().data = EvmByteArray::exact(&[3])
        .summary_projection(domain.borrowed())
        .unwrap();
    assert!(!covers(&left, &right));
    right = left.clone();
    let payload = right.possible_logs.remove(&key).unwrap();
    let mut foreign = key.clone();
    foreign.code_hash = alloy_primitives::B256::repeat_byte(1);
    right.possible_logs.insert(foreign, payload);
    assert!(!covers(&left, &right));
    left.logs_unknown = true;
    left.possible_logs.clear();
    assert!(covers(&left, &right));
    assert!(!covers(&right, &left));
}

#[test]
fn summary_coverage_optional_refusal_leaves_join_available_and_cancellation_propagates() {
    let _scope = CALLS.scope();
    let left = store();
    let mut right = store();
    right.balance_default = EvmValue::from_numeric(Native::private());
    let domain = ValueAlgebra::new(Policy {
        is_top_fee: usize::MAX,
        ..Policy::default()
    });
    let mut budget = WorkBudget::new(1_000_000);
    assert!(
        !left
            .covers_with_payment(&right, domain.borrowed(), &mut budget)
            .unwrap()
    );
    assert!(!budget.exhausted());
    assert!(
        left.join_with_payment(&right, domain.borrowed(), false, &mut budget)
            .is_ok()
    );
    let domain = ValueAlgebra::new(Policy::default());
    let mut exact = WorkBudget::new(1_000_000);
    assert!(
        left.covers_with_payment(&left, domain.borrowed(), &mut exact)
            .unwrap()
    );
    let fee = exact.used();
    for limit in [0, fee - 1, fee] {
        let mut budget = WorkBudget::new(limit);
        assert_eq!(
            left.covers_with_payment(&left, domain.borrowed(), &mut budget)
                .unwrap(),
            limit == fee
        );
        assert!(!budget.exhausted());
    }
    for captured in [false, true] {
        let cancellation = analysis_control::Cancellation::new();
        let mut budget = if captured {
            analysis_control::with_cancellation(&cancellation, || WorkBudget::new(1_000_000))
        } else {
            WorkBudget::new(1_000_000)
        };
        cancellation.cancel();
        CALLS.set(Calls::default());
        let mut run = || left.covers_with_payment(&right, domain.borrowed(), &mut budget);
        let result = if captured {
            run()
        } else {
            analysis_control::with_cancellation(&cancellation, run)
        };
        assert_eq!(result, Err(CloneError::Cancelled));
        assert_eq!(budget.used(), 0);
        assert_eq!(CALLS.get(), Calls::default());
    }
}
