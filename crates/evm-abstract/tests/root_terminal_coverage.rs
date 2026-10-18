//! Public root coverage keeps one input valuation separate from detailed evidence.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{
        self, ExecutionConfig, FrontierReason, Limit, OutcomeKind, RootTerminalScope,
        RootTerminalTrigger, Status,
        layers::AnalysisSession,
    },
    domain::{
        AbstractValue, Domain,
        symbolic::{ByteStateKind, ExprId, ExprKind, ExprLimits, SlotStateId, SlotStateKind, SourceId},
    },
    ssa::{self, PartialBlockCoverage},
    world::{Account, ByteArray, Code, Entry, Existence, Store, World},
};
use std::collections::BTreeMap;

const TWO_BLOCK_RETURN: &str = "6003565b602a5f5260205ff3";

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn fixture(fork: Fork) -> (World, Entry, ExecutionConfig) {
    let mut world = World::new(fork, "public root terminal coverage");
    for (owner, code) in [(0x101, TWO_BLOCK_RETURN), (0x202, "00")] {
        let mut account = Account::from_hex(code, fork).unwrap();
        account.storage_unknown = true;
        account.balance = word(10);
        account.nonce = word(1);
        world.insert(address(owner), account).unwrap();
    }
    world.insert(address(3), Account::empty()).unwrap();
    world.insert(address(0x303), Account::empty()).unwrap();
    let mut entry = Entry::new(address(0x101));
    entry.environment.caller = address(0x900).into();
    entry.environment.value = word(0);
    let mut config = ExecutionConfig::default();
    // Zero is rejected by Config::validate. One real transfer executes only
    // PUSH1/JUMP, then the second block meets the actual Transfers boundary.
    config.analysis.max_transfers = 1;
    (world, entry, config)
}

/// A finite witness for the Exact/Input/ordered-slot grammar used below.
/// Each immutable source gets one assignment across input, saved and output
/// roots. Unsupported shapes refuse; numerical marginals alone cannot pass.
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
            _ => false,
        }
    }

    fn slot(&mut self, state: &SlotStateId, key: U256, expected: U256) -> bool {
        match state.kind() {
            SlotStateKind::Zero => expected == U256::ZERO,
            SlotStateKind::Input { source, bindings } => {
                if let Some(binding) = bindings.iter().find(|binding| binding.index == key)
                    && (!binding.numeric.contains(expected)
                        || !self.expression(&binding.value, expected))
                {
                    return false;
                }
                Self::assign(&mut self.cells, (source.clone(), key), expected)
            }
            SlotStateKind::Store {
                previous,
                key: written,
                value,
            } => match written.as_constant() {
                Some(written) if written == key => self.expression(value, expected),
                Some(_) => self.slot(previous, key, expected),
                None => false,
            },
            SlotStateKind::Ite { .. } => false,
        }
    }

    fn storage(&mut self, store: &Store, owner: Address, expected: u64, domain: Domain) -> bool {
        let value = store.read_checked(owner, &word(8), domain).unwrap();
        let expected = U256::from(expected);
        value.contains(expected)
            && value
                .expression()
                .is_some_and(|expression| self.expression(expression, expected))
    }

    fn data(&mut self, data: &ByteArray, expected: &[u8]) -> bool {
        if !data.len().contains(U256::from(expected.len())) {
            return false;
        }
        match data.state_root().kind() {
            ByteStateKind::Exact(bytes) => bytes.as_ref() == expected,
            ByteStateKind::Input {
                source,
                length,
                bindings,
            } if bindings.is_empty() => {
                self.expression(length, U256::from(expected.len()))
                    && expected.iter().enumerate().all(|(index, byte)| {
                        Self::assign(
                            &mut self.cells,
                            (source.clone(), U256::from(index)),
                            U256::from(*byte),
                        )
                    })
            }
            _ => false,
        }
    }
}

#[test]
fn transfers_boundary_binds_the_original_request_without_completing_detail() {
    let (world, entry, mut config) = fixture(Fork::Osaka);
    assert!(!config.root_terminal_coverage);
    assert_eq!(config.max_work, 20_000_000);
    let ordinary = analysis::analyze_world(world.clone(), entry.clone(), config.clone()).unwrap();
    assert!(ordinary.root_terminal_coverage().is_none());
    assert!(
        serde_json::to_value(&ordinary)
            .unwrap()
            .get("root_terminal_coverage")
            .is_none()
    );

    config.root_terminal_coverage = true;
    let mut session =
        AnalysisSession::from_world(world.clone(), entry.clone(), config.clone()).unwrap();
    let snapshot = session.facts().snapshot().clone();
    let revision = session.facts().revision();
    let report = session.analyze().unwrap();
    let covered = report.analysis();
    let coverage = covered.root_terminal_coverage().unwrap();
    assert_eq!(coverage.scope(), RootTerminalScope::AllInputStates);
    assert_eq!(coverage.trigger(), &RootTerminalTrigger::Transfers);
    assert_eq!(
        coverage.outcomes().each_ref().map(|outcome| outcome.kind()),
        [OutcomeKind::Return, OutcomeKind::Revert, OutcomeKind::Failure]
    );

    let input = coverage.input();
    let original = &covered.states()[0].entry;
    let frame = original.active();
    assert_eq!(input.snapshot(), &snapshot);
    assert_eq!(input.snapshot(), report.snapshot());
    assert_eq!(input.revision(), revision);
    assert_eq!(input.revision(), report.revision());
    assert_eq!(input.fork(), world.fork());
    assert_eq!(input.entry(), &entry);
    assert_eq!(input.entry(), covered.entry());
    assert_eq!(input.domain_spec(), covered.domain_spec());
    assert_eq!(
        serde_json::to_value(input.config()).unwrap(),
        serde_json::to_value(&config).unwrap()
    );
    assert_eq!(input.frame(), &frame.key);
    assert_eq!(input.program(), frame.program.as_ref().unwrap());
    let Code::Runtime(request_program) = &world.accounts()[&entry.address].code else {
        panic!("fixture has runtime code");
    };
    assert_eq!(input.program(), request_program);
    assert_eq!(input.calldata(), &frame.calldata);
    assert_eq!(input.call_value(), &frame.call_value);
    assert_eq!(input.call_value().singleton(), Some(U256::ZERO));
    assert_eq!(input.saved_store(), &frame.saved_store);
    assert_eq!(input.initial_store(), &original.store);
    let ExprKind::Input(length) = input.calldata().len().expression().unwrap().kind() else {
        panic!("unknown calldata retains its original scoped input");
    };
    assert_eq!(input.input_scope(), Some(length.scope));
    let request_store = Store::new_checked(&world, Domain::from_spec(input.domain_spec())).unwrap();
    let SlotStateKind::Input { source, .. } = request_store.global_storage_state().kind() else {
        panic!("the world owns an immutable storage source");
    };
    assert_eq!(input.storage_source(), source.token());
    assert_eq!(input.saved_store().state(), &request_store);
    assert_eq!(
        input.initial_store().touched_in_transaction(entry.address),
        Some(true)
    );
    assert_eq!(
        input
            .saved_store()
            .state()
            .touched_in_transaction(entry.address),
        Some(false)
    );

    assert_eq!(covered.status(), Status::Incomplete);
    assert_eq!(covered.status(), ordinary.status());
    assert_eq!(covered.transfers(), 1);
    assert_eq!(covered.states().len(), 2);
    assert_eq!(covered.states().len(), ordinary.states().len());
    for (after, before) in covered.states().iter().zip(ordinary.states()) {
        assert_eq!(after.key, before.key);
        assert_eq!(after.entry, before.entry);
        assert_eq!(after.exit, before.exit);
        assert_eq!(after.executed_pcs, before.executed_pcs);
    }
    assert_eq!(covered.states()[0].executed_pcs, [0, 2]);
    assert!(covered.states()[0].execution_evidence().is_some());
    assert!(covered.states()[1].executed_pcs.is_empty());
    assert!(covered.states()[1].execution_evidence().is_none());
    assert_eq!(covered.frontiers().len(), 1);
    assert_eq!(covered.frontiers().len(), ordinary.frontiers().len());
    assert_eq!(
        covered.frontiers()[0].reason,
        FrontierReason::Budget(Limit::Transfers)
    );
    assert_eq!(covered.frontiers()[0].reason, ordinary.frontiers()[0].reason);
    assert_eq!(covered.frontiers()[0].pc, ordinary.frontiers()[0].pc);
    assert_eq!(covered.outcomes().len(), 1);
    assert_eq!(covered.outcomes()[0].kind, OutcomeKind::Failure);
    assert_eq!(covered.outcomes()[0].store, ordinary.outcomes()[0].store);
    assert_eq!(covered.outcomes()[0].data, ByteArray::empty());
    let partial = ssa::build_partial_world(covered).unwrap();
    partial.verify(covered).unwrap();
    assert_eq!(partial.status(), Status::Incomplete);
    assert_eq!(partial.frontiers().len(), 1);
    assert_eq!(partial.blocks().len(), 2);
    assert_eq!(partial.blocks()[0].instructions.len(), 2);
    assert_eq!(partial.blocks()[1].coverage, PartialBlockCoverage::Unexecuted);
    assert!(partial.blocks()[1].instructions.is_empty());
    assert!(ssa::build_world(covered).is_err());

    let native = serde_json::to_value(covered).unwrap();
    assert_eq!(analysis::SCHEMA_VERSION, 10);
    assert_eq!(native["schema_version"], 10);
    assert_eq!(
        native["status"],
        serde_json::to_value(Status::Incomplete).unwrap()
    );
    assert_eq!(native["outcomes"].as_array().unwrap().len(), 1);
    assert_eq!(native["root_terminal_coverage"]["scope"], "AllInputStates");
    assert_eq!(
        native["root_terminal_coverage"]["outcomes"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn one_certificate_admits_two_whole_input_valuations_and_independent_return_effects() {
    let (world, entry, mut config) = fixture(Fork::Osaka);
    config.root_terminal_coverage = true;
    let analysis = analysis::analyze_world(world, entry, config).unwrap();
    let coverage = analysis.root_terminal_coverage().unwrap();
    let input = coverage.input();
    let [returned, reverted, failed] = coverage.outcomes();
    let domain = Domain::from_spec(input.domain_spec());
    let history = input
        .initial_store()
        .slot_root(address(0x101), false)
        .unwrap();
    assert_ne!(
        returned.store().global_storage_state(),
        input.initial_store().global_storage_state()
    );
    for initial in [1, 7] {
        let mut joint = JointWitness::default();
        for (owner, value) in [(0x101, initial), (0x202, initial), (0x999, initial + 1)] {
            assert!(joint.storage(input.initial_store(), address(owner), value, domain));
            assert!(joint.storage(input.saved_store().state(), address(owner), value, domain));
            assert!(joint.storage(reverted.store(), address(owner), value, domain));
            assert!(joint.storage(failed.store(), address(owner), value, domain));
        }
        assert!(joint.data(input.calldata(), &[0x11, 0x22]));
        for (owner, value) in [(0x101, 31), (0x202, 47), (0x999, 93)] {
            assert!(joint.storage(returned.store(), address(owner), value, domain));
            // These scalar fields deliberately form Cartesian Top choices.
            assert!(
                returned
                    .store()
                    .read_balance(address(owner))
                    .numeric()
                    .is_top()
            );
            assert!(returned.store().nonce(address(owner)).numeric().is_top());
            assert!(matches!(
                returned.store().code(address(owner)),
                None | Some(Code::Unknown)
            ));
            assert_eq!(returned.store().existence(address(owner)), Existence::Unknown);
            for outcome in coverage.outcomes() {
                assert_eq!(
                    outcome
                        .store()
                        .read_transient(address(owner), &word(8), domain)
                        .singleton(),
                    Some(U256::ZERO)
                );
                assert_eq!(
                    outcome.store().created_in_transaction(address(owner)),
                    Some(false)
                );
                assert_eq!(
                    outcome.store().pending_destruction(address(owner)),
                    Some(false)
                );
            }
        }
        assert!(joint.data(returned.data(), &[0xa1, 0xb2, 0xc3]));
        assert!(joint.data(reverted.data(), &[0xd4, 0xe5]));
        assert!(joint.data(failed.data(), &[]));
        assert!(!joint.data(failed.data(), &[1]));
        assert!(joint.slot(&history, U256::from(8), U256::from(initial)));
        assert!(!joint.storage(input.initial_store(), address(0x101), 31, domain));
        assert!(!joint.storage(reverted.store(), address(0x101), 31, domain));
        assert!(!joint.storage(failed.store(), address(0x101), 31, domain));
        assert!(!joint.storage(returned.store(), address(0x101), 47, domain));
        assert!(!joint.data(input.calldata(), &[0xa1, 0xb2]));
    }
    assert!(returned.store().balance_default().numeric().is_top());
    assert!(returned.store().nonce_default().numeric().is_top());
    assert!(returned.store().code_default().is_none());
    assert_eq!(returned.store().existence_default(), Existence::Unknown);
    assert!(returned.store().global_storage_default().numeric().is_top());
    assert_eq!(
        returned.store().global_transient_default().singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(returned.store().created_default(), Some(false));
    assert_eq!(returned.store().pending_destruction_default(), Some(false));
    assert_eq!(returned.store().touched_default(), Some(false));
    // This is an any-sequence wildcard, including repeated logs, rather than
    // evidence that a LOG instruction or a particular receipt was observed.
    assert!(returned.store().logs_unknown());
    assert!(returned.store().possible_logs().is_empty());
    for outcome in [reverted, failed] {
        assert!(!outcome.store().logs_unknown());
        assert!(outcome.store().possible_logs().is_empty());
        assert_eq!(
            outcome.store().read_balance(address(0x101)).singleton(),
            Some(U256::from(10))
        );
        assert_eq!(
            outcome.store().nonce(address(0x101)).singleton(),
            Some(U256::from(1))
        );
    }
    let unsupported = ExprId::ite(
        ExprId::fresh().unwrap(),
        ExprId::constant(U256::from(1)),
        ExprId::constant(U256::from(7)),
        ExprLimits::default(),
    )
    .unwrap();
    assert!(matches!(unsupported.kind(), ExprKind::Ite { .. }));
    assert!(!JointWitness::default().expression(&unsupported, U256::from(1)));
}
