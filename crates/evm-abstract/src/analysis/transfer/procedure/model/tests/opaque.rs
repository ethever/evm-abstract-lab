//! Disable sharing to measure actual native copies/bytes on the same table.
use super::*;
use crate::{
    domain::{EvmValue, symbolic::ExprId},
    resource::clone_payment::tests::{RECEIPT, Receipt, Word, reset, summary_fixture},
};

#[test]
fn repeated_table_snapshot_copies_no_native_bytes_and_preserves_private_predicates() {
    let _scope = RECEIPT.scope();
    let (world, entry, mut config, algebra) = summary_fixture("60005460005260206000f3");
    config.max_work = 100_000_000;
    config.use_summaries = true;
    let (_, _, _, table_algebra) = summary_fixture("00");
    let mut table = Table::with_algebra(table_algebra);
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let value =
        EvmValue::from_numeric(Word::expensive(4096)).with_expression(ExprId::fresh().unwrap());
    let owner = entry.address;
    input.store.write(
        owner,
        &EvmValue::constant(U256::ZERO),
        &value,
        algebra.borrowed(),
    );
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
    assert!(solver.table.frontiers.is_empty());
    let (first, cfg, generation) = solver.semantic_snapshot().unwrap();
    reset();
    let start = solver.budget.used();
    let (shared, shared_cfg, shared_generation) = solver.semantic_snapshot().unwrap();
    let shared_work = solver.budget.used() - start;
    assert_eq!(RECEIPT.get(), Receipt::default());
    assert!(first.shares_storage_with(&shared));
    assert!(cfg.shares_storage_with(&shared_cfg));
    assert_eq!(generation, shared_generation);
    // Controlled same-table rebuild, with unchanged inputs, versions and budget.
    solver.table.model_rows.clear();
    solver.table.model_snapshot = None;
    solver.table.regional_snapshot = None;
    reset();
    let start = solver.budget.used();
    let (rebuilt, _, _) = solver.semantic_snapshot().unwrap();
    let rebuild_work = solver.budget.used() - start;
    let copies = RECEIPT.get();
    assert!(copies.clones > 0 && copies.bytes > 0, "{copies:?}");
    assert!(rebuild_work > shared_work);
    assert!(!first.shares_storage_with(&rebuilt));
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&rebuilt).unwrap()
    );
    for rows in [&first, &shared, &rebuilt] {
        let output = rows[0]
            .outputs
            .iter()
            .find(|o| o.kind == OutcomeKind::Return)
            .unwrap();
        let data = output
            .data
            .instantiate_with_payment(algebra.borrowed(), solver.budget)
            .unwrap();
        let word = data.read_word(&EvmValue::constant(U256::ZERO), algebra.borrowed());
        assert!(word.contains(U256::from(8)));
        assert!(
            !word.contains(U256::from(7)),
            "opaque private exclusion must survive"
        );
    }
    eprintln!(
        "G5 repeated-table control: shared_work={shared_work} native_clones=0 native_bytes=0; rebuild_work={rebuild_work} native_clones={} native_bytes={}",
        copies.clones, copies.bytes
    );
}
