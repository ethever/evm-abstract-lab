use super::*;
use crate::domain::symbolic::ExprId;
use revm_bytecode::opcode;

fn report_execution(case: &str, mode: &str, execution: &Execution, domain: Domain) -> usize {
    let mut returns = 0;
    for (index, output) in execution.outcomes.iter().enumerate() {
        let value = output.data.read_word(&word(0), domain);
        println!(
            "RELATION_BOUNDARY_PROBE case={case} mode={mode} outcome={index} kind={:?} length={:?} word={value:?} numeric={:?}",
            output.kind,
            output.data.len().numeric(),
            value.numeric(),
        );
        if output.kind == OutcomeKind::Return {
            returns += 1;
            if [
                "sameblock",
                "crossblock",
                "two-jump",
                "xor-sameblock",
                "xor-crossblock",
            ]
            .contains(&case)
            {
                assert_eq!(output.data.len().singleton(), Some(U256::from(32)));
                let expected = if case.starts_with("xor-") { 0 } else { 7 };
                assert_eq!(value.singleton(), Some(U256::from(expected)));
            }
        }
    }
    returns
}

#[test]
fn relation_boundary_probe() {
    // Direct Table admission is a controlled internal payload probe. These
    // programs do not contain recursive calls.
    let config = crate::analysis::ExecutionConfig::default();
    let domain = config.domain().unwrap();
    for (case, code) in [
        ("sameblock", "5f358060079055545f5260205ff3"),
        ("crossblock", "5f358060079055600a565b545f5260205ff3"),
        ("two-jump", "5f358060079055600a565b600e565b545f5260205ff3"),
        ("xor-sameblock", "5f3580185f5260205ff3"),
        ("xor-crossblock", "5f35806006565b185f5260205ff3"),
    ] {
        let (world, mut entry) = fixture(&[(0x101, code)]);
        entry.environment.calldata = ByteArray::unknown();
        let input =
            crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
        let mut table = Table::default();
        let mut table_budget = WorkBudget::new(config.max_work);
        let mut counters = Counters::default();
        let (execution, record) = table.execute(
            0,
            &world,
            &entry,
            input.clone(),
            &config,
            domain,
            &mut table_budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        );
        let table_returns = report_execution(case, "table", &execution, domain);
        assert!(record.closed && execution.frontiers.is_empty());
        println!(
            "RELATION_BOUNDARY_PROBE case={case} mode=table closed={} frontiers={:?} procedures={} states={} work={} transfers={} propagations={} reused={}",
            record.closed,
            execution.frontiers,
            record.procedures.len(),
            record.state_count,
            table_budget.used(),
            record.transfers,
            record.propagations,
            record.reused,
        );
        for (procedure, summary) in record.procedures.iter().enumerate() {
            for (index, output) in summary.outputs.iter().enumerate() {
                let value = output.data.read_word(&word(0), domain);
                println!(
                    "RELATION_BOUNDARY_PROBE case={case} mode=table procedure={procedure} output={index} kind={:?} length={:?} word={value:?} numeric={:?}",
                    output.kind,
                    output.data.len().numeric(),
                    value.numeric(),
                );
            }
        }

        let mut budget = WorkBudget::new(config.max_work);
        let mut queue = VecDeque::from([input]);
        let mut frontiers = Vec::new();
        let mut transfers = 0;
        let mut ordinary_returns = 0;
        while transfers < 1024 {
            let Some(payload) = queue.pop_front() else {
                break;
            };
            let execution = execute_inner(
                &world,
                &entry,
                payload,
                &config,
                domain,
                &mut budget,
                StorageReadPolicy::Abstract,
                false,
            );
            transfers += 1;
            ordinary_returns += report_execution(case, "ordinary", &execution, domain);
            frontiers.extend(execution.frontiers);
            queue.extend(
                execution
                    .successors
                    .into_iter()
                    .map(|successor| successor.payload),
            );
        }
        let host_step_cap = !queue.is_empty();
        for payload in queue {
            // A host worklist boundary, not an EVM Failure or an invented Return.
            frontiers.push(ExecutionFrontier {
                pc: 0,
                reason: FrontierReason::Budget(Limit::Transfers),
                target: Some(payload.key()),
                range: None,
                payload: Some(Arc::new(payload)),
            });
        }
        println!(
            "RELATION_BOUNDARY_PROBE case={case} mode=ordinary closed={} host_step_cap={host_step_cap} frontiers={frontiers:?} procedures=0 work={} transfers={transfers}",
            frontiers.is_empty(),
            budget.used(),
        );
        // Initial gas Failure remains in the printed outcomes; only actual
        // Return outcomes satisfy these assertions. Precision is observational.
        assert!(table_returns > 0, "{case}: Table produced no Return");
        assert!(
            ordinary_returns > 0,
            "{case}: ordinary execution produced no Return"
        );
    }
}

#[test]
fn relation_boundary_probe_same_numeric_different_alias_inputs() {
    // The real prefix loads k and q from distinct calldata words, then jumps
    // to block 1. The two prepared inputs share those same expression IDs;
    // only the path constraint differs. Preparation is outside probe ledgers.
    let code = "5f356020356008565b600782559050545f5260205ff3";
    let (world, mut entry) = fixture(&[(0x101, code)]);
    entry.environment.calldata = ByteArray::unknown();
    let config = crate::analysis::ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let initial =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    let mut preparation_budget = WorkBudget::new(config.max_work);
    let preparation = execute_inner(
        &world,
        &entry,
        initial,
        &config,
        domain,
        &mut preparation_budget,
        StorageReadPolicy::Abstract,
        false,
    );
    assert!(preparation.frontiers.is_empty());
    assert_eq!(preparation.successors.len(), 1);
    let input = preparation.successors.into_iter().next().unwrap().payload;
    assert_eq!(input.active().stack.len(), 2);
    let k = input.active().stack[0].clone();
    let q = input.active().stack[1].clone();
    assert_eq!(k.numeric(), q.numeric());
    let equality = ExprId::operation(
        opcode::EQ,
        &[
            k.expression().unwrap().clone(),
            q.expression().unwrap().clone(),
        ],
        config.analysis.relations.expressions(),
    )
    .unwrap();
    let mut inputs = Vec::new();
    for (case, alias, expected) in [("alias-equal", true, 7u64), ("alias-distinct", false, 0u64)] {
        let mut payload = input.clone();
        payload
            .relations
            .assume_checked(&equality, alias, &config.analysis.relations)
            .unwrap();
        assert!(!payload.relations.is_bottom());
        inputs.push((case, expected, payload));
    }
    let equal_form = form(&inputs[0].2, &entry, domain).unwrap();
    let distinct_form = form(&inputs[1].2, &entry, domain).unwrap();
    assert!(semantic_eq(&equal_form, &distinct_form));

    let mut shared = Table::default();
    for (case, expected, input) in inputs {
        let mut budget = WorkBudget::new(config.max_work);
        let ordinary = execute_inner(
            &world,
            &entry,
            input.clone(),
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
            false,
        );
        assert!(report_execution(case, "ordinary", &ordinary, domain) > 0);
        assert!(ordinary.outcomes.iter().any(|output| {
            output.kind == OutcomeKind::Return
                && output
                    .data
                    .read_word(&word(0), domain)
                    .contains(U256::from(expected))
        }));
        println!(
            "RELATION_BOUNDARY_PROBE case={case} mode=ordinary closed={} frontiers={:?} work={} transfers=1",
            ordinary.frontiers.is_empty(),
            ordinary.frontiers,
            budget.used(),
        );
        for mode in ["shared-table", "fresh-table"] {
            let mut fresh = Table::default();
            let table = if mode == "shared-table" {
                &mut shared
            } else {
                &mut fresh
            };
            let mut budget = WorkBudget::new(config.max_work);
            let mut counters = Counters::default();
            let (execution, record) = table.execute(
                0,
                &world,
                &entry,
                input.clone(),
                &config,
                domain,
                &mut budget,
                &mut counters,
                StorageReadPolicy::Abstract,
            );
            assert!(report_execution(case, mode, &execution, domain) > 0);
            // A broad reused invariant may cover both inputs. Reusing a narrow
            // value 7 for the nonalias input would exclude its concrete return 0.
            assert!(execution.outcomes.iter().any(|output| {
                output.kind == OutcomeKind::Return
                    && output
                        .data
                        .read_word(&word(0), domain)
                        .contains(U256::from(expected))
            }));
            println!(
                "RELATION_BOUNDARY_PROBE case={case} mode={mode} closed={} frontiers={:?} procedures={} states={} work={} transfers={} reused={}",
                record.closed,
                execution.frontiers,
                record.procedures.len(),
                record.state_count,
                budget.used(),
                record.transfers,
                record.reused,
            );
        }
    }
}

#[test]
fn storage_observation_known_delegatecall_write_replaces_current_snapshot() {
    // Controlled Table probe: the parent retains slot 1 across JUMP, then the
    // child writes 9 in its storage namespace. Revert must preserve actual 7;
    // success must preserve actual 9 in one joint data+Store outcome.
    let parent = "60018060079055600a565b5f5f5f5f6102015af450545f5260205ff3";
    for (case, child, expected) in [
        ("success", "60096001555f5ff3", 9),
        ("revert", "60096001555f5ffd", 7),
    ] {
        let (world, entry) = fixture(&[(0x101, parent), (0x201, child)]);
        let config = crate::analysis::ExecutionConfig::default();
        let domain = config.domain().unwrap();
        let input =
            crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
        let mut table = Table::default();
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
        println!(
            "STORAGE_CALL_PROBE case={case} closed={} frontiers={:?} work={} transfers={}",
            record.closed,
            execution.frontiers,
            budget.used(),
            record.transfers
        );
        assert!(record.closed && execution.frontiers.is_empty());
        assert!(
            record.procedures.len() >= 2,
            "known child must execute through Table"
        );
        for output in execution
            .outcomes
            .iter()
            .filter(|out| out.kind == OutcomeKind::Return)
        {
            let value = output.data.read_word(&word(0), domain);
            let stored = output
                .store
                .read_checked(address(0x101), &word(1), domain)
                .unwrap();
            println!(
                "STORAGE_CALL_PROBE case={case} data_singleton={:?} store_singleton={:?} data_constants={:?} store_constants={:?} data_top={} store_top={}",
                value.singleton(),
                stored.singleton(),
                value.constants(),
                stored.constants(),
                value.numeric().is_top(),
                stored.numeric().is_top()
            );
            assert_eq!(output.data.len().singleton(), Some(U256::from(32)));
        }
        assert!(
            execution.outcomes.iter().any(|output| {
                output.kind == OutcomeKind::Return
                    && output
                        .data
                        .read_word(&word(0), domain)
                        .contains(U256::from(expected))
                    && output
                        .store
                        .read_checked(address(0x101), &word(1), domain)
                        .unwrap()
                        .contains(U256::from(expected))
            }),
            "a joint parent outcome must retain the actual {case} value {expected}"
        );
    }
}

#[test]
fn storage_duplicate_default_work_probe() {
    // Both programs must close under the default budget with the exact read.
    // The second keeps 16 DUP aliases through all 20 unconditional JUMPs.
    let config = crate::analysis::ExecutionConfig::default();
    assert_eq!(config.max_work, 20_000_000);
    let domain = config.domain().unwrap();
    for copies in [1usize, 16] {
        let mut code = vec![0x5f, 0x35];
        code.extend(std::iter::repeat_n(0x80, copies));
        code.extend([0x60, 0x07, 0x90, 0x55]);
        for _ in 0..20 {
            let target = u16::try_from(code.len() + 4).unwrap();
            code.extend([0x61, (target >> 8) as u8, target as u8, 0x56, 0x5b]);
        }
        code.extend(std::iter::repeat_n(0x50, copies - 1));
        code.extend([0x54, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3]);
        let hex: String = code.iter().map(|byte| format!("{byte:02x}")).collect();
        println!("STORAGE_DUPLICATE_CODE copies={copies} jumps=20 hex={hex}");
        let (world, mut entry) = fixture(&[(0x101, hex.as_str())]);
        entry.environment.calldata = ByteArray::unknown();
        let input =
            crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
        let mut table = Table::default();
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
        let frontiers: Vec<_> = execution
            .frontiers
            .iter()
            .map(|f| (&f.reason, f.pc))
            .collect();
        let returns: Vec<_> = execution
            .outcomes
            .iter()
            .filter(|o| o.kind == OutcomeKind::Return)
            .map(|o| {
                let value = o.data.read_word(&word(0), domain);
                (
                    o.data.len().singleton(),
                    value.singleton(),
                    value.contains(U256::from(7)),
                    value.numeric().clone(),
                )
            })
            .collect();
        println!(
            "STORAGE_DUPLICATE_RESULT copies={copies} jumps=20 max_work={} closed={} frontiers={frontiers:?} returns={returns:?} work={} transfers={} states={} procedures={}",
            config.max_work,
            record.closed,
            budget.used(),
            record.transfers,
            record.state_count,
            record.procedures.len()
        );
        assert!(record.closed && execution.frontiers.is_empty());
        assert!(!returns.is_empty());
        assert!(
            returns
                .iter()
                .all(|r| r.0 == Some(U256::from(32)) && r.1 == Some(U256::from(7)))
        );
        assert!(returns.iter().all(|r| r.2));
    }
}
