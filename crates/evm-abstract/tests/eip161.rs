//! EIP-161 clearing uses transaction-scoped touches, including child rollback.
//! The concrete oracle checks revm's touch/empty state; the deletion decision
//! then follows EIP-161, rather than treating every observed empty account dead.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, OutcomeKind, Status, WorldAnalysis, analyze_world},
    domain::{AbstractValue, Domain},
    world::{Account, ByteArray, Entry, Existence, Store, World},
};
use revm::{
    Context, InspectEvm, Inspector, MainBuilder, MainContext,
    context::{TxEnv, result::ExecutionResult},
    database::InMemoryDB,
    interpreter::{Interpreter, interpreter::EthInterpreter, interpreter_types::Jumps},
    primitives::{Bytes, TxKind},
    state::{AccountInfo, Bytecode},
};
use std::collections::BTreeSet;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn fixture(code: &str, target: Account) -> World {
    let mut world = World::new(Fork::Osaka, "issue134:eip161");
    world
        .insert(
            address(0x101),
            Account::from_hex(code, world.fork()).unwrap(),
        )
        .unwrap();
    world.insert(address(0x200), target).unwrap();
    world.insert(address(0x300), Account::empty()).unwrap();
    world
}

fn run(world: World, summaries: bool) -> WorldAnalysis {
    let mut config = ExecutionConfig {
        max_work: 100_000_000,
        use_summaries: summaries,
        ..ExecutionConfig::default()
    };
    config.analysis.relations.enabled = false;
    let mut entry = Entry::new(address(0x101));
    entry.environment.caller = address(0x1000).into();
    entry.environment.value = word(0);
    entry.environment.calldata = ByteArray::empty();
    let analysis = analyze_world(world, entry, config).unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    analysis
}

fn returns(analysis: &WorldAnalysis) -> impl Iterator<Item = &Store> {
    analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .map(|outcome| &outcome.store)
}

#[test]
fn only_touched_definitely_empty_accounts_are_definitely_cleared() {
    let domain = Domain::default();
    let mut world = fixture("00", Account::empty());
    let mut unknown_balance = Account::empty();
    unknown_balance.balance = domain.join(&word(0), &word(7));
    world.insert(address(0x400), unknown_balance).unwrap();
    let mut unknown_nonce = Account::empty();
    unknown_nonce.nonce = domain.join(&word(0), &word(1));
    world.insert(address(0x500), unknown_nonce).unwrap();
    world.insert(address(0x600), Account::absent()).unwrap();
    let mut nonempty = Account::empty();
    nonempty.balance = word(1);
    world.insert(address(0x700), nonempty).unwrap();
    let mut store = Store::new_checked(&world, domain).unwrap();
    for number in [0x200, 0x400, 0x500, 0x600, 0x700] {
        store.touch_account(address(number));
    }
    store.finalize_transaction(domain);
    assert_eq!(store.existence(address(0x200)), Existence::Absent);
    assert_eq!(
        store.existence(address(0x300)),
        Existence::Present,
        "untouched empty account"
    );
    assert_eq!(store.existence(address(0x400)), Existence::Unknown);
    assert_eq!(store.existence(address(0x500)), Existence::Unknown);
    assert!(store.read_balance(address(0x400)).contains(U256::from(7)));
    assert!(store.nonce(address(0x500)).contains(U256::ONE));
    assert_eq!(store.existence(address(0x600)), Existence::Absent);
    assert_eq!(store.existence(address(0x700)), Existence::Present);
    for number in [0x200, 0x300, 0x400, 0x500, 0x600, 0x700, 0x800] {
        assert_eq!(store.touched_in_transaction(address(number)), Some(false));
    }
    let finalized = store.clone();
    store.finalize_transaction(domain);
    assert_eq!(
        store, finalized,
        "clearing must not leak into a later transaction"
    );
}

#[test]
fn touch_may_must_joins_and_summary_widening_keep_both_outcomes() {
    let domain = Domain::default();
    let world = fixture("00", Account::empty());
    let untouched = Store::new_checked(&world, domain).unwrap();
    assert_eq!(untouched.touched_default(), Some(false));
    let mut touched = untouched.clone();
    touched.touch_account(address(0x200));
    for (left, right) in [(&touched, &untouched), (&untouched, &touched)] {
        let joined = left.join(right, domain);
        assert_eq!(joined.touched_in_transaction(address(0x200)), None);
        assert_eq!(joined.touched_in_transaction(address(0x300)), Some(false));
        let mut finalized = joined.clone();
        finalized.finalize_transaction(domain);
        assert_eq!(finalized.existence(address(0x200)), Existence::Unknown);
        assert_eq!(finalized.existence(address(0x300)), Existence::Present);
        for widen in [false, true] {
            let summary = left.summary_projection(domain).unwrap().join(
                &right.summary_projection(domain).unwrap(),
                domain,
                widen,
            );
            let mut replay = summary.instantiate(domain).unwrap();
            assert_eq!(replay.touched_in_transaction(address(0x200)), None);
            assert_eq!(replay.touched_in_transaction(address(0x300)), Some(false));
            replay.finalize_transaction(domain);
            assert_eq!(replay.existence(address(0x200)), Existence::Unknown);
            assert_eq!(replay.existence(address(0x300)), Existence::Present);
        }
    }
    let must = touched
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    assert_eq!(must.touched_in_transaction(address(0x200)), Some(true));
    let mut all_may = untouched.clone();
    all_may.may_touch_all();
    assert_eq!(all_may.touched_default(), None);
    let replay = all_may
        .summary_projection(domain)
        .unwrap()
        .instantiate(domain)
        .unwrap();
    assert_eq!(
        replay.touched_in_transaction(address(0x999)),
        None,
        "unlisted recipient default"
    );
    let joined = untouched.join(&replay, domain);
    assert_eq!(joined.touched_in_transaction(address(0x999)), None);
    assert_eq!(replay.touched_default(), None);
    assert_eq!(joined.touched_default(), None);
}

#[test]
fn snapshots_roll_back_touches_except_the_ripemd_consensus_exception() {
    let domain = Domain::default();
    let world = fixture("00", Account::empty());
    let mut store = Store::new_checked(&world, domain).unwrap();
    store.touch_account(address(0x200));
    let before = store.clone();
    let saved = store.snapshot();
    store.touch_account(address(0x300));
    store.may_touch_account(address(0x400));
    store.restore(saved);
    assert_eq!(store, before);
    store.finalize_transaction(domain);
    assert_eq!(store.existence(address(0x200)), Existence::Absent);
    assert_eq!(store.existence(address(0x300)), Existence::Present);

    let mut store = Store::new_checked(&world, domain).unwrap();
    let saved = store.snapshot();
    store.touch_account(address(3));
    store.write_balance(address(3), word(9));
    store.restore(saved);
    assert_eq!(store.touched_in_transaction(address(3)), Some(true));
    assert!(
        store.read_balance(address(3)).numeric().is_top(),
        "touch survives, value effect does not"
    );
}

#[test]
fn unknown_effects_preserve_must_touches_and_static_value_state() {
    let domain = Domain::default();
    let world = fixture("00", Account::empty());
    for is_static in [false, true] {
        let mut store = Store::new_checked(&world, domain).unwrap();
        store.touch_account(address(0x200));
        let before = store.clone();
        store
            .apply_unknown_effects(&BTreeSet::new(), None, is_static, world.fork())
            .unwrap();
        assert_eq!(store.touched_in_transaction(address(0x200)), Some(true));
        assert_eq!(store.touched_in_transaction(address(0x300)), None);
        assert_eq!(store.touched_in_transaction(address(0x999)), None);
        if is_static {
            let mut expected = before;
            expected.may_touch_all();
            assert_eq!(store, expected);
            store.finalize_transaction(domain);
            assert_eq!(store.existence(address(0x200)), Existence::Absent);
            assert_eq!(store.existence(address(0x300)), Existence::Unknown);
        }
    }
    let mut created = Store::new_checked(&world, domain).unwrap();
    created.begin_creation(address(0x300));
    assert_eq!(created.touched_in_transaction(address(0x300)), Some(true));
    created.finalize_transaction(domain);
    assert_eq!(
        created.existence(address(0x300)),
        Existence::Present,
        "created account nonce is one"
    );

    let mut destroyed = Store::new_checked(&world, domain).unwrap();
    destroyed.selfdestruct(address(0x101), address(0x200), domain);
    assert_eq!(destroyed.touched_in_transaction(address(0x200)), Some(true));
    destroyed.finalize_transaction(domain);
    assert_eq!(
        destroyed.existence(address(0x200)),
        Existence::Absent,
        "zero-balance SELFDESTRUCT touches its beneficiary"
    );
    assert_eq!(destroyed.existence(address(0x300)), Existence::Present);
}

#[test]
fn actual_call_clears_empty_recipient_and_failed_entry_does_not_touch_it() {
    for summaries in [false, true] {
        // The Issue134 bytecode, including a separate immediate-failure route.
        let analysis = run(
            fixture("5f5f5f5f5f61020062fffffff15000", Account::empty()),
            summaries,
        );
        assert!(
            returns(&analysis).any(|store| store.existence(address(0x200)) == Existence::Absent)
        );
        assert!(
            returns(&analysis).any(|store| store.existence(address(0x200)) == Existence::Present)
        );
        assert!(
            returns(&analysis).all(|store| store.existence(address(0x300)) == Existence::Present)
        );

        // Balance zero cannot fund value one: no successful child touch exists.
        let analysis = run(
            fixture("5f5f5f5f600161020062fffffff15000", Account::empty()),
            summaries,
        );
        assert!(returns(&analysis).next().is_some());
        assert!(
            returns(&analysis).all(|store| store.existence(address(0x200)) == Existence::Present)
        );

        let analysis = run(
            fixture("5f5f5f5f5f61020062fffffff15000", Account::absent()),
            summaries,
        );
        assert!(
            returns(&analysis).all(|store| store.existence(address(0x200)) == Existence::Absent)
        );

        let mut uncertain = Account::empty();
        uncertain.balance = Domain::default().join(&word(0), &word(7));
        let analysis = run(
            fixture("5f5f5f5f5f61020062fffffff15000", uncertain),
            summaries,
        );
        assert!(
            returns(&analysis).any(|store| store.existence(address(0x200)) == Existence::Unknown)
        );
        assert!(
            returns(&analysis)
                .all(|store| store.read_balance(address(0x200)).contains(U256::from(7)))
        );
    }
}

#[test]
fn root_and_child_reverts_restore_their_own_touch_scope() {
    for summaries in [false, true] {
        let analysis = run(
            fixture("5f5f5f5f5f61020062fffffff1505f5ffd", Account::empty()),
            summaries,
        );
        assert!(
            analysis
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == OutcomeKind::Revert)
        );
        assert!(
            analysis
                .outcomes()
                .iter()
                .all(|outcome| outcome.store.existence(address(0x200)) == Existence::Present)
        );

        let child = Account::from_hex("5f5f5f5f5f61030062fffffff1505f5ffd", Fork::Osaka).unwrap();
        let analysis = run(fixture("5f5f5f5f5f61020062fffffff15000", child), summaries);
        assert!(returns(&analysis).next().is_some());
        assert!(
            returns(&analysis).all(|store| store.existence(address(0x300)) == Existence::Present)
        );

        let mut empty_root = fixture("", Account::empty());
        empty_root.insert(address(0x900), Account::empty()).unwrap();
        let analysis = run(empty_root, summaries);
        assert!(
            returns(&analysis).any(|store| store.existence(address(0x101)) == Existence::Absent)
        );
        assert!(
            returns(&analysis).all(|store| store.existence(address(0x900)) == Existence::Present)
        );
        assert!(analysis.outcomes().iter().any(|outcome| {
            outcome.kind == OutcomeKind::Failure
                && outcome.store.existence(address(0x101)) == Existence::Present
                && outcome.store.touched_in_transaction(address(0x101)) == Some(false)
        }));
    }
}

#[derive(Default)]
struct Trace {
    calls: usize,
}

impl<CTX> Inspector<CTX, EthInterpreter> for Trace {
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut CTX) {
        if matches!(interpreter.bytecode.opcode(), 0xf1 | 0xfa) {
            self.calls += 1;
        }
    }
}

#[test]
fn revm_touch_and_rollback_agree_with_eip161_abstract_outcomes() {
    for (code, target, expected_touch) in [
        ("5f5f5f5f5f61020062fffffff15000", 0x200, true),
        ("5f5f5f5f600161020062fffffff15000", 0x200, false),
        ("5f5f5f5f5f61020062fffffff1505f5ffd", 0x200, false),
        ("5f5f5f5f61020062fffffffa5000", 0x200, true),
        // Zero gas makes RIPEMD fail after its recipient touch. That touch is
        // the specific consensus exception retained by upstream checkpoint_revert.
        ("5f5f5f5f5f60035ff15000", 3, true),
    ] {
        let mut world = fixture(code, Account::empty());
        if target != 0x200 {
            world.insert(address(target), Account::empty()).unwrap();
        }
        let mut db = InMemoryDB::default();
        for (owner, account) in world.accounts() {
            let bytecode = Bytecode::new_raw(Bytes::from(world.raw_account_code(*owner).unwrap()));
            db.insert_account_info(
                *owner,
                AccountInfo::new(
                    account.balance.singleton().unwrap(),
                    account.nonce.singleton().unwrap().to::<u64>(),
                    bytecode.hash_slow(),
                    bytecode,
                ),
            );
        }
        db.insert_account_info(
            address(0x1000),
            AccountInfo {
                balance: U256::from(100_000_000),
                ..AccountInfo::default()
            },
        );
        let context = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(world.fork().spec_id()))
            .with_db(db);
        let mut evm = context.build_mainnet_with_inspector(Trace::default());
        let concrete = evm
            .inspect_tx(
                TxEnv::builder()
                    .caller(address(0x1000))
                    .kind(TxKind::Call(address(0x101)))
                    .gas_limit(10_000_000)
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            evm.inspector.calls, 1,
            "oracle must execute the witness CALL"
        );
        let recipient = concrete.state.get(&address(target)).unwrap();
        assert_eq!(recipient.is_touched(), expected_touch);
        assert!(recipient.info.is_empty());
        let expected = if expected_touch {
            Existence::Absent
        } else {
            Existence::Present
        };
        let kind = match concrete.result {
            ExecutionResult::Success { .. } => OutcomeKind::Return,
            ExecutionResult::Revert { .. } => OutcomeKind::Revert,
            ExecutionResult::Halt { .. } => OutcomeKind::Failure,
        };
        let analysis = run(world, true);
        assert!(
            analysis
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == kind
                    && outcome.store.existence(address(target)) == expected)
        );
    }
}

#[test]
fn static_unknown_callback_touches_survive_the_relative_summary() {
    for summaries in [false, true] {
        let world = fixture("5f5f5f5f61020062fffffffa5000", Account::unknown());
        let analysis = run(world, summaries);
        assert!(
            returns(&analysis)
                .any(|store| { store.existence(address(0x300)) == Existence::Unknown }),
            "static unknown can touch the empty account, or return without touching it"
        );
    }
}
