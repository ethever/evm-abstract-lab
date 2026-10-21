use super::super::{
    StorageReadPolicy, WorkBudget, execute, initial,
    query::tests::{Policy, RECEIPT, Word},
};
use crate::{
    Fork, U256,
    analysis::{EnginePolicies, ExecutionConfig, FrontierReason, Status},
    domain::{
        EvmValue, ProductAlgebra, ValueAlgebra, WordElement, identity::Symbol, provenance::Origin,
        relational::QueryReason, symbolic::ExprId,
    },
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use alloy_primitives::Address;

fn tagged(word: u64) -> EvmValue<Word> {
    EvmValue::from_numeric(Word {
        query_tag: Some(700),
        ..Word::constant(U256::from(word))
    })
}

fn fixture(code: &str) -> (EvmWorld<Word>, EvmEntry<Word>, ExecutionConfig) {
    let owner = Address::repeat_byte(0x71);
    let mut world = EvmWorld::new(Fork::Osaka, "environment query probe");
    world
        .insert(
            owner,
            EvmAccount::<Word>::from_hex(code, Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let config = ExecutionConfig {
        max_work: 10_000_000,
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    };
    (world, entry, config)
}

#[test]
fn production_environment_queries_have_an_explicit_payer() {
    let _receipt = RECEIPT.scope();
    let algebra = ValueAlgebra::new(Policy);
    let mut observations = Vec::new();
    for (name, code) in [
        ("initial-call-value", "00"),
        ("timestamp", "425000"),
        ("block-number", "6006405000"),
    ] {
        let (world, mut entry, config) = fixture(code);
        match name {
            "initial-call-value" => entry.environment.value = tagged(7),
            "timestamp" => entry.environment.timestamp = tagged(7),
            _ => entry.environment.number = tagged(7),
        }
        RECEIPT.update(|r| {
            r.query_tag = Some(700);
            r.fee = usize::MAX;
            r.quotes = 0;
            r.queries = 0;
        });
        let result = crate::analysis::AnalysisEngine::new(
            &algebra,
            &EnginePolicies::default(),
            &mut crate::analysis::cooperation::NoExtension,
        )
        .run(
            crate::analysis::AnalysisInput::new(world, entry).unwrap(),
            config,
            &crate::analysis::control::Control::default(),
        )
        .unwrap();
        let r = RECEIPT.get();
        eprintln!(
            "baseline {name}: status={:?} work={} quotes={} queries={} fee=MAX frontiers={:?}",
            result.status(),
            result.work(),
            r.quotes,
            r.queries,
            result.frontiers()
        );
        assert_eq!(
            result.status(),
            if name == "block-number" {
                Status::Incomplete
            } else {
                Status::Converged
            }
        );
        assert_eq!(r.quotes, usize::from(name == "block-number"));
        observations.push((name, r.quotes, r.queries));
    }
    let (world, mut entry, config) = fixture("4000");
    entry.environment.number =
        EvmValue::constant(U256::from(8)).with_expression(ExprId::constant(U256::from(8)));
    let mut payload = initial(&world, &entry, algebra.borrowed()).unwrap();
    payload.active_mut().stack.push(tagged(7));
    RECEIPT.update(|r| {
        r.query_tag = Some(700);
        r.fee = usize::MAX;
        r.quotes = 0;
        r.queries = 0;
    });
    let mut budget = WorkBudget::new(config.max_work);
    let result = execute(
        &world,
        &entry,
        payload,
        &config,
        algebra.borrowed(),
        &crate::analysis::RecentDistinctJumps,
        &mut budget,
        StorageReadPolicy::Abstract,
    );
    let r = RECEIPT.get();
    eprintln!(
        "baseline block-index: work={} quotes={} queries={} fee=MAX frontiers={:?}",
        budget.used(),
        r.quotes,
        r.queries,
        result.frontiers
    );
    observations.push(("block-index", r.quotes, r.queries));
    assert_eq!(r.quotes, 1);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert!(
        result.outcomes.is_empty()
            && result.successors.is_empty()
            && result.instruction_values.is_empty()
    );
    assert!(
        observations.iter().all(|(_, _, queries)| *queries == 0),
        "unauthorized MAX singleton calls: {observations:?}"
    );
}

#[test]
fn explicit_symbol_binding_preserves_opaque_value_and_metadata_without_discovery() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|r| {
        r.query_tag = Some(700);
        r.fee = usize::MAX;
        r.cancel_on_query = true;
    });
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let before = tagged(7)
            .with_origin(Origin::Calldata)
            .with_identity(31, 4)
            .with_symbolic_limit();
        let bound = before.clone().with_symbol(Symbol::Timestamp, Some(19));
        assert_eq!(bound.numeric(), before.numeric());
        assert_eq!(bound.provenance(), before.provenance());
        assert!(bound.same_identity(&before));
        assert!(bound.symbolic_limit_reached());
        assert_eq!(bound.identity().input().unwrap().scope(), 19);
        assert_eq!(
            bound.identity().input().unwrap().symbol(),
            Symbol::Timestamp
        );
        assert_eq!(
            bound.expression(),
            Some(&ExprId::input(19, Symbol::Timestamp))
        );
        let prior = before.clone().with_expression(ExprId::fresh().unwrap());
        assert_eq!(
            prior
                .clone()
                .with_symbol(Symbol::Number, Some(20))
                .expression(),
            prior.expression()
        );
        assert_eq!(prior.clone().with_symbol(Symbol::Number, None), prior);
        assert!(!token.is_cancelled());
    });
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
}

fn watched_setup() {
    RECEIPT.update(|r| {
        r.query_tag = Some(700);
        r.fee = 61;
        r.quotes = 0;
        r.queries = 0;
    });
}

#[test]
fn production_blob_hash_queries_have_an_explicit_payer() {
    let _receipt = RECEIPT.scope();
    let algebra = ValueAlgebra::new(Policy);
    let mut observed = Vec::new();
    for watch_length in [true, false] {
        let (world, mut entry, config) = fixture("4900");
        entry.environment.blob_hashes.length = if watch_length {
            tagged(7)
        } else {
            EvmValue::constant(U256::from(8)).with_expression(ExprId::constant(U256::from(8)))
        };
        let mut payload = initial(&world, &entry, algebra.borrowed()).unwrap();
        payload.active_mut().stack.push(if watch_length {
            EvmValue::constant(U256::from(6)).with_expression(ExprId::constant(U256::from(6)))
        } else {
            tagged(7)
        });
        watched_setup();
        RECEIPT.update(|r| r.fee = usize::MAX);
        let mut budget = WorkBudget::new(config.max_work);
        let result = execute(
            &world,
            &entry,
            payload,
            &config,
            algebra.borrowed(),
            &crate::analysis::RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        let r = RECEIPT.get();
        eprintln!(
            "production blob {}: work={} quotes={} queries={} fee=MAX frontiers={:?}",
            if watch_length { "length" } else { "index" },
            budget.used(),
            r.quotes,
            r.queries,
            result.frontiers
        );
        assert_eq!(r.quotes, 1);
        assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        assert!(
            result.outcomes.is_empty()
                && result.successors.is_empty()
                && result.instruction_values.is_empty()
        );
        observed.push((r.quotes, r.queries));
    }
    assert!(
        observed.iter().all(|(_, queries)| *queries == 0),
        "unowned BLOBHASH queries: {observed:?}"
    );
}

#[test]
fn block_hash_prepays_each_opaque_input_once_and_refuses_atomically() {
    let _receipt = RECEIPT.scope();
    let algebra = ValueAlgebra::new(Policy);
    for watch_number in [true, false] {
        let (_, mut entry, _) = fixture("4000");
        entry.environment.number = if watch_number {
            tagged(7)
        } else {
            EvmValue::constant(U256::from(8)).with_expression(ExprId::constant(U256::from(8)))
        };
        let index = if watch_number {
            EvmValue::constant(U256::from(6)).with_expression(ExprId::constant(U256::from(6)))
        } else {
            tagged(7)
        };
        let queried_index = if watch_number { 6 } else { 7 };
        entry
            .environment
            .block_hashes
            .insert(U256::from(queried_index), U256::from(42).into());
        let envelope = algebra.operation_work(revm_bytecode::opcode::BLOCKHASH, &[]);
        for (fee, remaining, allowed) in [
            (61, 61, true),
            (61, 60, false),
            (usize::MAX, usize::MAX - envelope, false),
        ] {
            watched_setup();
            RECEIPT.update(|r| r.fee = fee);
            let mut budget = WorkBudget::new(envelope.saturating_add(remaining));
            assert!(budget.charge(envelope));
            let out =
                super::block_hash(&entry.environment, &index, algebra.borrowed(), &mut budget);
            if allowed {
                let out = out.unwrap();
                assert_eq!(
                    (out.numeric().lower, out.numeric().upper),
                    (U256::from(42), U256::from(42))
                );
            } else {
                assert_eq!(out, Err(FrontierReason::Work));
            }
            assert_eq!(
                (RECEIPT.get().quotes, RECEIPT.get().queries),
                (1, usize::from(allowed))
            );
            assert_eq!(budget.used(), envelope + if allowed { 61 } else { 0 });
        }
    }
}

#[test]
fn block_hash_checks_current_and_captured_cancellation_at_each_query_boundary() {
    let _receipt = RECEIPT.scope();
    let algebra = ValueAlgebra::new(Policy);
    for stage in 0..3 {
        for captured in [false, true] {
            watched_setup();
            let owner = analysis_control::Cancellation::new();
            let active = analysis_control::Cancellation::new();
            let (_, mut entry, _) = fixture("4000");
            let mut number = tagged(7).numeric().clone();
            if captured && stage == 1 {
                number.cancel_quote_owner = Some(owner.clone());
            }
            if captured && stage == 2 {
                number.cancel_query_owner = Some(owner.clone());
            }
            entry.environment.number = EvmValue::from_numeric(number);
            let index =
                EvmValue::constant(U256::from(6)).with_expression(ExprId::constant(U256::from(6)));
            let mut budget =
                analysis_control::with_cancellation(&owner, || WorkBudget::new(1_000_000));
            let envelope = algebra.operation_work(revm_bytecode::opcode::BLOCKHASH, &[]);
            assert!(budget.charge(envelope));
            if stage == 0 {
                if captured {
                    owner.cancel();
                } else {
                    active.cancel();
                }
            }
            RECEIPT.update(|r| {
                r.cancel_on_quote = !captured && stage == 1;
                r.cancel_on_query = !captured && stage == 2;
            });
            analysis_control::with_cancellation(&active, || {
                assert_eq!(
                    super::block_hash(&entry.environment, &index, algebra.borrowed(), &mut budget),
                    Err(FrontierReason::Relations(QueryReason::Cancelled))
                );
            });
            assert_eq!(
                (RECEIPT.get().quotes, RECEIPT.get().queries),
                (usize::from(stage > 0), usize::from(stage == 2))
            );
            assert_eq!(budget.used(), envelope + if stage == 2 { 61 } else { 0 });
        }
    }
}

#[test]
fn block_hash_cached_boundary_proofs_avoid_unnecessary_opaque_discovery() {
    let _receipt = RECEIPT.scope();
    watched_setup();
    RECEIPT.update(|r| r.fee = usize::MAX);
    let algebra = ValueAlgebra::new(Policy);
    let (_, mut entry, _) = fixture("4000");
    entry.environment.number = tagged(7);
    let max_index = EvmValue::constant(U256::MAX).with_expression(ExprId::constant(U256::MAX));
    let mut budget = WorkBudget::new(100_000);
    assert!(budget.charge(algebra.operation_work(revm_bytecode::opcode::BLOCKHASH, &[])));
    let out = super::block_hash(
        &entry.environment,
        &max_index,
        algebra.borrowed(),
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        (out.numeric().lower, out.numeric().upper),
        (U256::ZERO, U256::ZERO)
    );
    entry.environment.number =
        EvmValue::constant(U256::ZERO).with_expression(ExprId::constant(U256::ZERO));
    let out = super::block_hash(
        &entry.environment,
        &tagged(7),
        algebra.borrowed(),
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        (out.numeric().lower, out.numeric().upper),
        (U256::ZERO, U256::ZERO)
    );
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
}

#[test]
fn block_hash_keeps_recent_window_and_finite_or_unknown_index_semantics() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let mut env = crate::analysis::testing::EvmEnvironment {
        number: crate::analysis::testing::TestValue::constant(U256::from(300)),
        ..crate::analysis::testing::EvmEnvironment::default()
    };
    for index in [0, 43, 44, 299, 300, 301] {
        env.block_hashes
            .insert(U256::from(index), U256::from(42).into());
    }
    for (index, expected) in [(0, 0), (43, 0), (44, 42), (299, 42), (300, 0), (301, 0)] {
        let mut budget = WorkBudget::new(1_000_000);
        assert!(budget.charge(algebra.operation_work(revm_bytecode::opcode::BLOCKHASH, &[])));
        let out = super::block_hash(
            &env,
            &crate::analysis::testing::TestValue::constant(U256::from(index)),
            algebra.borrowed(),
            &mut budget,
        )
        .unwrap();
        assert_eq!(out.singleton(), Some(U256::from(expected)), "index={index}");
    }
    let finite = algebra.join(
        &crate::analysis::testing::TestValue::constant(U256::from(44)),
        &crate::analysis::testing::TestValue::constant(U256::from(300)),
    );
    let mut budget = WorkBudget::new(1_000_000);
    assert!(budget.charge(algebra.operation_work(revm_bytecode::opcode::BLOCKHASH, &[]) * 2));
    let out = super::block_hash(&env, &finite, algebra.borrowed(), &mut budget).unwrap();
    for word in 0..=50 {
        assert_eq!(out.contains(U256::from(word)), matches!(word, 0 | 42));
    }
    let unknown = crate::analysis::testing::TestValue::top();
    let out = super::block_hash(&env, &unknown, algebra.borrowed(), &mut budget).unwrap();
    assert!(out.numeric().is_top());
    env.number = unknown.clone();
    let out = super::block_hash(
        &env,
        &crate::analysis::testing::TestValue::constant(U256::from(44)),
        algebra.borrowed(),
        &mut budget,
    )
    .unwrap();
    assert!(out.contains(U256::ZERO) && out.contains(U256::from(42)));
}

#[test]
fn blob_hash_pays_length_or_index_once_with_atomic_refusal_and_cancellation() {
    let _receipt = RECEIPT.scope();
    let algebra = ValueAlgebra::new(Policy);
    for watch_length in [true, false] {
        for mode in 0..5 {
            watched_setup();
            RECEIPT.update(|r| {
                r.cancel_on_quote = mode == 3;
                r.cancel_on_query = mode == 4;
                r.fee = if mode == 1 { usize::MAX } else { 61 };
            });
            let (_, mut entry, _) = fixture("4900");
            entry.environment.blob_hashes.length = if watch_length {
                tagged(7)
            } else {
                EvmValue::constant(U256::from(8)).with_expression(ExprId::constant(U256::from(8)))
            };
            let index = if watch_length {
                EvmValue::constant(U256::from(6)).with_expression(ExprId::constant(U256::from(6)))
            } else {
                tagged(7)
            };
            entry.environment.blob_hashes.hashes.insert(
                U256::from(if watch_length { 6 } else { 7 }),
                U256::from(42).into(),
            );
            let envelope = algebra.operation_work(revm_bytecode::opcode::BLOBHASH, &[]);
            let mut budget = WorkBudget::new(envelope + if mode == 2 { 60 } else { 61 });
            assert!(budget.charge(envelope));
            let token = analysis_control::Cancellation::new();
            let out = analysis_control::with_cancellation(&token, || {
                super::blob_hash(
                    &entry.environment.blob_hashes,
                    &index,
                    entry.environment.input_scope.id(),
                    algebra.borrowed(),
                    &mut budget,
                )
            });
            match mode {
                0 => {
                    let out = out.unwrap();
                    assert_eq!(
                        (out.numeric().lower, out.numeric().upper),
                        (U256::from(42), U256::from(42))
                    );
                }
                1 | 2 => assert_eq!(out, Err(FrontierReason::Work)),
                _ => assert_eq!(out, Err(FrontierReason::Relations(QueryReason::Cancelled))),
            }
            assert_eq!(
                (RECEIPT.get().quotes, RECEIPT.get().queries),
                (1, usize::from(matches!(mode, 0 | 4)))
            );
            assert_eq!(
                budget.used(),
                envelope + if matches!(mode, 0 | 4) { 61 } else { 0 }
            );
        }
    }
}

#[test]
fn cancellation_during_environment_discovery_publishes_no_transfer_effect() {
    let _receipt = RECEIPT.scope();
    let algebra = ValueAlgebra::new(Policy);
    for blob in [false, true] {
        for captured in [false, true] {
            let (world, mut entry, config) = fixture(if blob { "4900" } else { "4000" });
            let owner = analysis_control::Cancellation::new();
            let active = analysis_control::Cancellation::new();
            let mut word = tagged(7).numeric().clone();
            word.cancel_query_owner = captured.then(|| owner.clone());
            if blob {
                entry.environment.blob_hashes.length = EvmValue::from_numeric(word);
            } else {
                entry.environment.number = EvmValue::from_numeric(word);
            }
            let mut payload = initial(&world, &entry, algebra.borrowed()).unwrap();
            payload.active_mut().stack.push(
                EvmValue::constant(U256::from(6)).with_expression(ExprId::constant(U256::from(6))),
            );
            watched_setup();
            RECEIPT.update(|r| r.cancel_on_query = !captured);
            let mut budget =
                analysis_control::with_cancellation(&owner, || WorkBudget::new(config.max_work));
            let result = analysis_control::with_cancellation(&active, || {
                execute(
                    &world,
                    &entry,
                    payload,
                    &config,
                    algebra.borrowed(),
                    &crate::analysis::RecentDistinctJumps,
                    &mut budget,
                    StorageReadPolicy::Abstract,
                )
            });
            assert_eq!(
                result.frontiers[0].reason,
                FrontierReason::Relations(QueryReason::Cancelled)
            );
            assert!(
                result.outcomes.is_empty()
                    && result.successors.is_empty()
                    && result.instruction_values.is_empty()
            );
            assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
        }
    }
}

#[test]
fn blob_hash_keeps_closed_open_and_partial_table_semantics() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let mut env = crate::world::EvmBlobHashes::exact(&[U256::from(42).into()]);
    let mut budget = WorkBudget::new(1_000_000);
    assert!(budget.charge(algebra.operation_work(revm_bytecode::opcode::BLOBHASH, &[]) * 8));
    for (index, expected) in [(U256::ZERO, 42), (U256::ONE, 0), (U256::MAX, 0)] {
        let out = super::blob_hash(
            &env,
            &crate::analysis::testing::TestValue::constant(index),
            Some(4),
            algebra.borrowed(),
            &mut budget,
        )
        .unwrap();
        assert_eq!(out.singleton(), Some(U256::from(expected)));
    }
    env.length = crate::analysis::testing::TestValue::top();
    let out = super::blob_hash(
        &env,
        &crate::analysis::testing::TestValue::constant(U256::ZERO),
        Some(4),
        algebra.borrowed(),
        &mut budget,
    )
    .unwrap();
    assert_eq!(out.singleton(), Some(U256::from(42)));
    let out = super::blob_hash(
        &env,
        &crate::analysis::testing::TestValue::constant(U256::ONE),
        Some(4),
        algebra.borrowed(),
        &mut budget,
    )
    .unwrap();
    assert!(out.numeric().is_top());
    assert_eq!(
        out.identity().input().unwrap().symbol(),
        Symbol::BlobHash(U256::ONE)
    );
    let out = super::blob_hash(
        &env,
        &crate::analysis::testing::TestValue::top(),
        Some(4),
        algebra.borrowed(),
        &mut budget,
    )
    .unwrap();
    assert!(out.numeric().is_top());
    env.length = crate::analysis::testing::TestValue::constant(U256::ZERO);
    let out = super::blob_hash(
        &env,
        &crate::analysis::testing::TestValue::top(),
        Some(4),
        algebra.borrowed(),
        &mut budget,
    )
    .unwrap();
    assert_eq!(out.singleton(), Some(U256::ZERO));
}

#[test]
fn opaque_unknown_bound_reuses_its_paid_negative_answer() {
    let _receipt = RECEIPT.scope();
    let algebra = ValueAlgebra::new(Policy);
    for blob in [false, true] {
        let (_, mut entry, _) = fixture("4000");
        let unknown = EvmValue::from_numeric(Word {
            query_tag: Some(700),
            ..Word::top()
        });
        entry.environment.number = unknown.clone();
        entry.environment.blob_hashes.length = unknown;
        entry
            .environment
            .block_hashes
            .insert(U256::from(6), U256::from(42).into());
        entry
            .environment
            .blob_hashes
            .hashes
            .insert(U256::from(6), U256::from(42).into());
        let index =
            EvmValue::constant(U256::from(6)).with_expression(ExprId::constant(U256::from(6)));
        watched_setup();
        RECEIPT.update(|r| r.watched = None);
        let envelope = algebra.operation_work(
            if blob {
                revm_bytecode::opcode::BLOBHASH
            } else {
                revm_bytecode::opcode::BLOCKHASH
            },
            &[],
        );
        let mut budget = WorkBudget::new(envelope + 61);
        assert!(budget.charge(envelope));
        let out = if blob {
            super::blob_hash(
                &entry.environment.blob_hashes,
                &index,
                Some(1),
                algebra.borrowed(),
                &mut budget,
            )
        } else {
            super::block_hash(&entry.environment, &index, algebra.borrowed(), &mut budget)
        }
        .unwrap();
        assert!(out.contains(U256::from(42)));
        assert_eq!(out.contains(U256::ZERO), !blob);
        assert_eq!(
            (RECEIPT.get().quotes, RECEIPT.get().queries, budget.used()),
            (1, 1, envelope + 61)
        );
        // An opaque index with no finite cover must remain unknown, rather
        // than inventing either a table entry or an out-of-range zero.
        entry.environment.number =
            EvmValue::constant(U256::from(8)).with_expression(ExprId::constant(U256::from(8)));
        entry.environment.blob_hashes.length = entry.environment.number.clone();
        let index = EvmValue::from_numeric(Word {
            query_tag: Some(700),
            ..Word::top()
        });
        watched_setup();
        RECEIPT.update(|r| r.watched = None);
        let mut budget = WorkBudget::new(envelope + 61);
        assert!(budget.charge(envelope));
        let out = if blob {
            super::blob_hash(
                &entry.environment.blob_hashes,
                &index,
                Some(1),
                algebra.borrowed(),
                &mut budget,
            )
        } else {
            super::block_hash(&entry.environment, &index, algebra.borrowed(), &mut budget)
        }
        .unwrap();
        assert!(out.numeric().is_top());
        assert_eq!(
            (RECEIPT.get().quotes, RECEIPT.get().queries, budget.used()),
            (1, 1, envelope + 61)
        );
    }
}
