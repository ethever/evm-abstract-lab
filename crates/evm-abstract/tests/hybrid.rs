//! Baseline-first native execution, without changing the original Unknown input.
#![allow(dead_code)]

#[path = "hybrid/namespace.rs"]
mod namespace;
#[path = "support/native.rs"]
mod native;
#[path = "unknown_world/oracle.rs"]
mod oracle;
#[path = "unknown_world/support.rs"]
mod support;

use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, DetailState, EdgeKind, EnginePolicies, ExecutionConfig,
        HybridAnalysis, HybridConfig, HybridInterpretation, MachineEdgeKind, ProcessOwner,
        RuntimeExecution, Status, TransferSemantics, control::Control, cooperation::NoExtension,
    },
    domain::{EvmValue, NumericValue},
    world::{AddressInput, EvmByteArray, EvmEntry, EvmWorld},
};
use revm_bytecode::opcode;
use support::{A, Assembly, U, address, insert, new_world};

fn unknown() -> EvmEntry<NumericValue> {
    EvmEntry {
        address: address(A),
        environment: evm_abstract::world::EvmEnvironmentValues {
            to: address(A).into(),
            caller: AddressInput::unknown_caller(),
            value: EvmValue::top(),
            calldata: EvmByteArray::unknown_checked().unwrap(),
            ..Default::default()
        },
    }
}

fn run(world: EvmWorld<NumericValue>, work: usize, detail: usize) -> HybridAnalysis<NumericValue> {
    run_config(
        world,
        ExecutionConfig {
            max_work: work,
            ..Default::default()
        },
        HybridConfig {
            max_detail_work: detail,
            ..Default::default()
        },
    )
}

fn run_config(
    world: EvmWorld<NumericValue>,
    config: ExecutionConfig,
    hybrid: HybridConfig,
) -> HybridAnalysis<NumericValue> {
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let input = AnalysisInput::new(world, unknown()).unwrap();
    let original = input.entry().clone();
    let result = AnalysisEngine::new(&native::product(), &policies, &mut extension)
        .run_hybrid(input, config, hybrid, &Control::default())
        .unwrap();
    assert_eq!(result.baseline().entry(), &original);
    eprintln!(
        "HYBRID_JSON {}",
        serde_json::json!({
            "coverage": result.coverage(),
            "status": result.status(),
            "baseline_status": result.baseline().status(),
            "detail_status": result.detail().map(|r| r.status()),
            "detail_states": result.detail().map(|r| r.states().len()),
            "baseline_frontiers": result.baseline().frontiers().iter().map(|f| &f.reason).collect::<Vec<_>>(),
            "detail_frontiers": result.detail().map(|r| r.frontiers().iter().map(|f| &f.reason).collect::<Vec<_>>()),
        })
    );
    result
}

fn branch() -> support::Fixture {
    let mut a = Assembly::default();
    a.push(0).op(opcode::ISZERO).jump("yes", true);
    a.push(99).op(opcode::STOP);
    a.label("yes").push_label("done").op(opcode::JUMP);
    a.label("done").op(opcode::STOP);
    a.finish()
}

fn branch_world() -> EvmWorld<NumericValue> {
    let mut world = new_world();
    insert(&mut world, A, &branch(), &[(0, 7)]);
    world
}

fn assert_accounting(result: &HybridAnalysis<NumericValue>, maximum: usize) {
    let work = result.coverage().work;
    assert_eq!(
        result.work(),
        work.baseline + work.preparation + work.detail
    );
    assert_eq!(result.baseline().work(), work.baseline);
    assert!(work.preparation + work.detail <= work.detail_allowance);
    assert!(result.work() <= maximum);
    if let Some(detail) = result.detail() {
        assert_eq!(detail.work(), result.work());
        assert_eq!(detail.snapshot(), result.baseline().snapshot());
        assert_eq!(detail.revision(), result.baseline().revision());
        assert_eq!(
            detail.world().fingerprint().unwrap(),
            result.baseline().world().fingerprint().unwrap()
        );
        let mut expected = result.baseline().entry().clone();
        if let evm_abstract::analysis::HybridNativeBinding::SymbolicOwnerNamespace { namespace } =
            result.coverage().detail_binding
        {
            expected.environment.to = namespace.into();
        }
        assert_eq!(detail.entry(), &expected);
    }
}

#[test]
fn original_unknown_input_gets_closed_control_then_actual_native_jump_branch_detail() {
    let result = run(branch_world(), 100_000_000, 50_000_000);
    assert_eq!(result.status(), Status::Converged);
    assert!(result.coverage().baseline_current && result.coverage().detail_current);
    assert_eq!(result.coverage().chosen, HybridInterpretation::Native);
    assert!(result.baseline().root_terminal_coverage().is_none());
    assert!(result.chosen().root_terminal_coverage().is_none());
    assert_eq!(
        result.baseline().entry().environment.caller,
        AddressInput::unknown_caller()
    );
    assert!(
        result
            .baseline()
            .entry()
            .environment
            .value
            .contains(evm_abstract::U256::MAX)
    );
    assert!(
        result
            .baseline()
            .entry()
            .environment
            .calldata
            .len()
            .contains(evm_abstract::U256::MAX)
    );
    for record in result.baseline().procedure_summaries() {
        assert!(record.closed && record.source_current);
        for node in &record.regional_cfg.nodes {
            let receipt = node.transfer.as_ref().unwrap();
            assert!(!receipt.visited_block);
            assert!(receipt.process.is_some());
        }
    }
    let locals = result.local_detail().collect::<Vec<_>>();
    assert!(
        locals
            .iter()
            .any(|site| site.state.executed_pcs.contains(&7))
    );
    assert!(
        locals
            .iter()
            .all(|site| site.state.execution_evidence().unwrap().is_current())
    );
    let native = result.detail().unwrap();
    assert!(
        native
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Intraprocedural(EdgeKind::BranchTrue))
    );
    assert!(
        native
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Intraprocedural(EdgeKind::Jump))
    );
    assert!(
        !native
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Intraprocedural(EdgeKind::BranchFalse))
    );
    assert_accounting(&result, 100_000_000);
    let encoded = serde_json::to_value(&result).unwrap();
    assert_eq!(encoded["local_detail"][0]["scope"], "RetainedInputPrefix");
    assert!(encoded["local_detail"][0].get("state").is_none());
    assert!(encoded["local_detail"][0]["instructions"].is_array());
    assert!(encoded["local_detail"][0]["executed_pcs"].is_array());
}

#[test]
fn baseline_work_refusal_stays_incomplete_and_never_starts_native_detail() {
    let result = run(branch_world(), 1, 100_000_000);
    assert_eq!(result.status(), Status::Incomplete);
    assert_eq!(result.baseline().status(), Status::Incomplete);
    assert!(!result.baseline().frontiers().is_empty());
    assert_eq!(result.coverage().detail, DetailState::BaselinePending);
    assert!(result.detail().is_none());
    assert_eq!(
        result.coverage().work.preparation + result.coverage().work.detail,
        0
    );
    assert_accounting(&result, 1);
}

#[test]
fn detail_refusal_does_not_invalidate_certified_coarse_control() {
    let coarse = run(branch_world(), 100_000_000, 0);
    let refused = run(branch_world(), 100_000_000, 1);
    assert_eq!(coarse.status(), Status::Converged);
    assert_eq!(refused.status(), Status::Converged);
    assert!(refused.coverage().baseline_current);
    assert!(!refused.coverage().detail_current);
    assert_eq!(refused.coverage().detail, DetailState::Work);
    assert_eq!(refused.coverage().chosen, HybridInterpretation::Process);
    assert_eq!(refused.baseline().work(), coarse.baseline().work());
    assert!(refused.detail().is_none());
    assert_accounting(&refused, 100_000_000);
}

#[test]
fn native_detail_budget_boundaries_preserve_baseline_and_all_actual_spent() {
    for allowance in (16_000..100_000).step_by(2000) {
        let result = run(branch_world(), 100_000_000, allowance);
        assert!(result.coverage().baseline_current, "{allowance}");
        assert_eq!(result.status(), Status::Converged);
        assert_accounting(&result, 100_000_000);
    }
}

#[test]
fn same_declared_ledger_with_extra_work_improves_detail_without_rescuing_a_refusal() {
    let coarse = run(branch_world(), 100_000_000, 0);
    let exact = run(branch_world(), coarse.work(), 100_000_000);
    let larger = run(branch_world(), coarse.work() + 50_000_000, 100_000_000);
    assert!(exact.coverage().baseline_current);
    assert!(!exact.coverage().detail_current);
    assert_eq!(exact.work(), coarse.work());
    assert!(larger.coverage().detail_current);
    assert!(larger.local_detail().count() > exact.local_detail().count());
    assert_eq!(
        larger.coverage().work.baseline,
        coarse.coverage().work.baseline
    );
    assert_accounting(&exact, coarse.work());
    assert_accounting(&larger, coarse.work() + 50_000_000);
}

fn callback_world() -> EvmWorld<NumericValue> {
    let mut a = Assembly::default();
    a.input(0).op(opcode::ISZERO).jump("outer", true);
    a.push(1).op(opcode::STOP);
    a.label("outer")
        .call(opcode::CALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[(0, 0)]);
    // U is intentionally unlisted: no invented concrete source or owner fact.
    world
}

#[test]
fn closed_baseline_keeps_known_unknown_callbacks_and_unlisted_target_complement() {
    let world = callback_world();
    assert!(!world.accounts().contains_key(&address(U)));
    let result = run(world, 2_000_000_000, 0);
    assert!(
        result.coverage().baseline_current,
        "{:?}",
        result.coverage()
    );
    assert_eq!(result.status(), Status::Converged);
    let mut residual = false;
    let mut callback = false;
    for record in result.baseline().procedure_summaries() {
        assert!(record.closed && record.source_current);
        for call in &record.regional_cfg.calls {
            residual |= call.target == Some(ProcessOwner::Residual);
            let from = &record.regional_cfg.nodes[call.from];
            let target = &record.procedures[call.callee];
            callback |= from
                .frame
                .process
                .as_ref()
                .is_some_and(|key| key.owner == ProcessOwner::Residual)
                && target
                    .frame
                    .process
                    .as_ref()
                    .is_some_and(|key| key.owner == ProcessOwner::Named(address(A)))
                && !call.deliveries.is_empty();
        }
    }
    assert!(residual && callback);
    assert!(
        result
            .baseline()
            .procedure_summaries()
            .iter()
            .any(|record| record
                .procedures
                .iter()
                .any(|p| p.semantics == TransferSemantics::Environment))
    );
    assert_accounting(&result, 2_000_000_000);
}

fn multi_region_world() -> EvmWorld<NumericValue> {
    let mut parent = Assembly::default();
    parent.input(0).op(opcode::ISZERO).jump("slow", true);
    parent
        .call(opcode::STATICCALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .op(opcode::STOP);
    parent.label("slow");
    // Many captured blocks require real native transfers but the process
    // baseline covers their local computation with its paid collapsed rule.
    for _ in 0..96 {
        parent
            .op(opcode::JUMPDEST)
            .push(17)
            .push(19)
            .op(opcode::EXP)
            .op(opcode::POP);
    }
    parent.op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &parent.finish(), &[(0, 0)]);
    insert(&mut world, U, &return_dispatch(), &[]);
    world
}

fn return_dispatch() -> support::Fixture {
    let mut code = Assembly::default();
    code.push_label("return_one").jump("dispatch", false);
    code.label("return_one")
        .push_label("return_two")
        .jump("dispatch", false);
    code.label("return_two").op(opcode::STOP);
    code.label("dispatch").op(opcode::JUMP);
    code.finish()
}

#[test]
fn partial_native_sites_layer_on_closed_baseline_while_another_region_is_pending() {
    let coarse = run(multi_region_world(), 2_000_000_000, 0);
    let result = run_config(
        multi_region_world(),
        ExecutionConfig {
            max_work: 2_000_000_000,
            root_terminal_coverage: false,
            analysis: evm_abstract::analysis::Config {
                context_depth: 2,
                max_transfers: coarse.baseline().transfers() + 32,
                ..Default::default()
            },
            ..Default::default()
        },
        HybridConfig {
            max_detail_work: 2_000_000,
            max_detail_regions: 1,
        },
    );
    assert!(coarse.coverage().baseline_current && result.coverage().baseline_current);
    let detail = result.detail().expect("the original input was admitted");
    assert_eq!(detail.status(), Status::Incomplete);
    assert!(!detail.frontiers().is_empty());
    assert_eq!(result.status(), Status::Converged);
    assert!(!result.coverage().detail_current);
    assert_eq!(
        result.coverage().chosen,
        HybridInterpretation::ProcessWithDetail
    );
    let sites = result.local_detail().collect::<Vec<_>>();
    assert!(!sites.is_empty());
    assert!(
        sites
            .iter()
            .all(|site| site.state.active().jump_history.len() <= 2)
    );
    assert!(sites.iter().any(|site| {
        site.state
            .execution_evidence()
            .unwrap()
            .successors()
            .iter()
            .any(|(_, edge)| *edge == MachineEdgeKind::Call)
    }));
    assert!(detail.procedure_summaries().iter().any(|record| {
        record.regional_cfg.nodes.iter().any(|node| {
            node.frame.code_address == address(U)
                && node.transfer.as_ref().is_some_and(|transfer| {
                    transfer.current && transfer.complete && transfer.visited_block
                })
        })
    }));
    assert!(
        sites
            .iter()
            .any(|site| site.context_selected && !site.state.active().jump_history.is_empty())
    );
    assert!(
        sites
            .iter()
            .filter(|site| site.state.active().code_address == address(U))
            .all(|site| !site.context_selected && site.state.active().jump_history.is_empty())
    );
    assert!(coarse.local_detail().next().is_none());
    assert!(
        result
            .baseline()
            .procedure_summaries()
            .iter()
            .all(|r| r.closed && r.source_current)
    );
    assert_accounting(&result, 2_000_000_000);
}

#[test]
fn frozen_region_choice_changes_actual_native_keys_and_retains_owner_static_context() {
    let execute = |regions| {
        run_config(
            multi_region_world(),
            ExecutionConfig {
                max_work: 2_000_000_000,
                root_terminal_coverage: false,
                analysis: evm_abstract::analysis::Config {
                    context_depth: 2,
                    ..Default::default()
                },
                ..Default::default()
            },
            HybridConfig {
                max_detail_work: 50_000_000,
                max_detail_regions: regions,
            },
        )
    };
    let one = execute(1);
    let two = execute(2);
    assert!(one.coverage().detail_current && two.coverage().detail_current);
    assert_eq!(one.coverage().detail_regions.len(), 1);
    assert_eq!(two.coverage().detail_regions.len(), 2);
    assert_eq!(one.coverage().detail_regions[0].code_address, address(A));
    for result in [&one, &two] {
        for site in result.local_detail() {
            let key = site.state.active();
            assert_eq!(key.jump_history, site.state.entry.active().key.jump_history);
            assert!(key.jump_history.len() <= 2);
            if key.code_address == address(U) {
                assert_eq!(key.address, address(U));
                assert!(key.is_static);
                assert_eq!(
                    site.context_selected,
                    result.coverage().detail_regions.len() == 2
                );
                // The selected caller's context survives inside a coarse child.
                assert!(!site.state.key.frames[0].jump_history.is_empty());
            }
        }
        assert_accounting(result, 2_000_000_000);
    }
    let detail = one.detail().unwrap();
    let mut unselected_bodies = 0;
    for record in detail.procedure_summaries() {
        let source = &detail.states()[record.source_state];
        assert!(!source.key.frames[0].jump_history.is_empty());
        for node in &record.regional_cfg.nodes {
            if node.frame.code_address == address(U) {
                unselected_bodies += 1;
                assert_eq!(node.frame.address, address(U));
                assert!(node.frame.is_static);
                assert!(node.frame.jump_history.is_empty());
            }
        }
    }
    assert!(unselected_bodies > 0);
    assert!(
        two.local_detail()
            .any(|site| site.state.active().code_address == address(U)
                && !site.state.active().jump_history.is_empty())
    );
}

#[test]
fn selected_contexts_refine_joined_jump_targets_while_the_other_region_remains_pending() {
    let baseline = run(multi_region_world(), 2_000_000_000, 0);
    // Keep the long independent path pending even if a core quote becomes
    // cheaper. Context selection, not a particular tariff, is the discriminator.
    let transfer_cap = baseline.baseline().transfers() + 32;
    let execute = |regions| {
        run_config(
            multi_region_world(),
            ExecutionConfig {
                max_work: 2_000_000_000,
                root_terminal_coverage: false,
                analysis: evm_abstract::analysis::Config {
                    context_depth: 2,
                    max_transfers: transfer_cap,
                    ..Default::default()
                },
                ..Default::default()
            },
            HybridConfig {
                max_detail_work: 2_000_000,
                max_detail_regions: regions,
            },
        )
    };
    let coarse = execute(1);
    let precise = execute(2);
    let fixture = return_dispatch();
    let dispatch = fixture.labels["dispatch"] + 1;
    let targets = |result: &HybridAnalysis<NumericValue>| {
        let native = result.detail().unwrap();
        result
            .local_detail()
            .filter(|site| {
                site.state.active().code_address == address(U)
                    && site.state.executed_pcs.contains(&dispatch)
                    && site.frontiers().next().is_none()
            })
            .map(|site| {
                assert_eq!(
                    site.instructions.last(),
                    Some(&evm_abstract::analysis::InstructionProgress::Dispatched)
                );
                site.successors
                    .iter()
                    .filter(|(_, edge)| *edge == MachineEdgeKind::Intraprocedural(EdgeKind::Jump))
                    .map(|(to, _)| {
                        let state = &native.states()[*to];
                        state.program().unwrap().blocks()[state.active().basic_block_index]
                            .instructions[0]
                            .pc
                    })
                    .collect::<std::collections::BTreeSet<_>>()
            })
            .collect::<Vec<_>>()
    };
    for result in [&coarse, &precise] {
        assert_eq!(result.status(), Status::Converged);
        assert_eq!(
            result.coverage().chosen,
            HybridInterpretation::ProcessWithDetail
        );
        assert_eq!(result.detail().unwrap().status(), Status::Incomplete);
        assert!(!result.detail().unwrap().frontiers().is_empty());
        assert!(result.coverage().baseline_current);
        assert_accounting(result, 2_000_000_000);
    }
    // Unselected bodies execute in the relative table; their current transfer
    // receipts carry control edges without inventing World instruction PCs.
    let coarse_targets = coarse
        .detail()
        .unwrap()
        .procedure_summaries()
        .iter()
        .flat_map(|record| {
            record.regional_cfg.nodes.iter().filter_map(|node| {
                let transfer = node.transfer.as_ref()?;
                let source = record.procedures[node.procedure].frame.code_address;
                let evm_abstract::world::Code::Runtime(program) =
                    &coarse.detail().unwrap().world().accounts()[&source].code
                else {
                    return None;
                };
                let block = program.blocks().get(node.frame.basic_block_index)?;
                if node.frame.code_address != address(U)
                    || !block
                        .instructions
                        .iter()
                        .any(|instruction| instruction.pc == dispatch)
                    || !transfer.current
                    || !transfer.complete
                    || !transfer.visited_block
                {
                    return None;
                }
                Some(
                    transfer
                        .successors
                        .iter()
                        .filter(|edge| {
                            edge.kind == MachineEdgeKind::Intraprocedural(EdgeKind::Jump)
                        })
                        .map(|edge| {
                            let to = &record.regional_cfg.nodes[edge.to];
                            program.blocks()[to.frame.basic_block_index].instructions[0].pc
                        })
                        .collect::<std::collections::BTreeSet<_>>(),
                )
            })
        })
        .collect::<Vec<_>>();
    let precise_targets = targets(&precise);
    let expected = std::collections::BTreeSet::from([
        fixture.labels["return_one"],
        fixture.labels["return_two"],
    ]);
    assert!(
        coarse_targets.iter().any(|set| set == &expected),
        "{coarse_targets:?}"
    );
    assert!(precise_targets.len() >= 2, "{precise_targets:?}");
    assert!(precise_targets.iter().all(|set| set.len() == 1));
    assert_eq!(
        precise_targets
            .into_iter()
            .flatten()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
}

#[test]
fn delegated_root_selects_actual_code_source_before_unrelated_runtime() {
    let mut world = new_world();
    let implementation = 0x301;
    let mut account = evm_abstract::world::EvmAccount::empty();
    account.code = evm_abstract::world::Code::Delegation(address(implementation));
    world.insert(address(A), account).unwrap();
    insert(
        &mut world,
        U,
        &support::Fixture {
            bytes: vec![opcode::STOP],
            labels: Default::default(),
        },
        &[],
    );
    insert(&mut world, implementation, &branch(), &[]);
    let result = run(world, 100_000_000, 50_000_000);
    assert!(result.coverage().baseline_current && result.coverage().detail_current);
    assert_eq!(result.coverage().detail_regions.len(), 1);
    assert_eq!(
        result.coverage().detail_regions[0].code_address,
        address(implementation)
    );
    assert!(result.local_detail().any(|site| site.context_selected
        && site.state.active().address == address(A)
        && site.state.active().code_address == address(implementation)
        && !site.state.active().jump_history.is_empty()));
    assert_accounting(&result, 100_000_000);
}

#[test]
fn symbolic_root_native_currency_cannot_replace_alias_covering_process_relation() {
    let mut code = Assembly::default();
    code.store(0, 9).op(opcode::STOP);
    let code = code.finish();
    let mut world = new_world();
    insert(&mut world, A, &code, &[(0, 5)]);
    insert(&mut world, U, &code, &[(0, 5)]);
    let mut entry = unknown();
    entry.environment.to = AddressInput::unknown_to();
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let report = AnalysisEngine::new(&native::product(), &policies, &mut extension)
        .run_hybrid(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig {
                max_work: 100_000_000,
                ..ExecutionConfig::default()
            },
            HybridConfig::default(),
            &Control::default(),
        )
        .unwrap();
    assert!(report.coverage().baseline_current);
    assert!(report.coverage().detail_current);
    assert_eq!(
        report.coverage().detail_binding,
        evm_abstract::analysis::HybridNativeBinding::SymbolicOwnerNamespace {
            namespace: address(A)
        }
    );
    assert!(
        report
            .baseline()
            .entry()
            .environment
            .to
            .as_concrete()
            .is_none()
    );
    assert_eq!(
        report
            .detail()
            .unwrap()
            .entry()
            .environment
            .to
            .as_concrete(),
        Some(address(A))
    );
    assert_eq!(
        report.coverage().chosen,
        HybridInterpretation::ProcessWithDetail
    );
    assert!(std::ptr::eq(report.chosen(), report.baseline()));
    assert!(
        report.local_detail().all(|site| site.scope
            == evm_abstract::analysis::HybridLocalScope::NativeOwnerNamespacePrefix)
    );
    let joint = |analysis: &evm_abstract::analysis::EvmWorldAnalysis<NumericValue>| {
        analysis.outcomes().iter().any(|outcome| {
            outcome.kind == evm_abstract::analysis::OutcomeKind::Return
                && support::read(&outcome.store, A, 0).contains(evm_abstract::U256::from(5))
                && support::read(&outcome.store, U, 0).contains(evm_abstract::U256::from(9))
        })
    };
    eprintln!(
        "HYBRID_ALIAS_JSON {}",
        serde_json::json!({
            "native_current": report.coverage().detail_current, "chosen":report.coverage().chosen,
            "binding":report.coverage().detail_binding, "native_joint_a5_u9":joint(report.detail().unwrap()),
            "baseline_joint_a5_u9":joint(report.baseline()), "chosen_joint_a5_u9":joint(report.chosen()),
        })
    );
    assert!(joint(report.baseline()));
    assert!(joint(report.chosen()));
    assert!(matches!(
        report.coverage().detail_regions[0].source,
        evm_abstract::analysis::ProcessSource::InputProgram { .. }
    ));
    let region = serde_json::to_value(report.coverage().detail_regions[0]).unwrap();
    assert!(region["source"].get("InputProgram").is_some());
    assert!(region.get("code_address").is_none());
    assert_accounting(&report, 100_000_000);
}

#[test]
fn auxiliary_native_envelope_never_certifies_native_or_process_completion() {
    let baseline = run(branch_world(), 100_000_000, 0);
    assert!(baseline.coverage().baseline_current);
    assert!(baseline.coverage().requested_native_terminal_coverage);
    assert!(baseline.baseline().root_terminal_coverage().is_none());
    assert!(baseline.detail().is_none());
    let transfers = baseline.baseline().transfers();
    for requested in [true, false] {
        let report = run_config(
            branch_world(),
            ExecutionConfig {
                max_work: 100_000_000,
                root_terminal_coverage: requested,
                analysis: evm_abstract::analysis::Config {
                    max_transfers: transfers,
                    ..Default::default()
                },
                ..Default::default()
            },
            HybridConfig::default(),
        );
        assert!(report.coverage().baseline_current);
        assert_eq!(report.status(), Status::Converged);
        assert_eq!(
            report.coverage().requested_native_terminal_coverage,
            requested
        );
        assert!(report.baseline().root_terminal_coverage().is_none());
        assert!(!report.baseline().config().root_terminal_coverage);
        let detail = report.detail().unwrap();
        assert_eq!(detail.config().root_terminal_coverage, requested);
        assert_eq!(detail.status(), Status::Incomplete);
        assert_eq!(detail.root_terminal_coverage().is_some(), requested);
        assert!(!report.coverage().detail_current);
        assert_eq!(report.coverage().chosen, HybridInterpretation::Process);
        assert!(report.local_detail().next().is_none());
        assert_accounting(&report, 100_000_000);
    }
}

#[test]
fn summary_cache_opt_out_keeps_actual_process_and_native_interpretations() {
    let report = run_config(
        branch_world(),
        ExecutionConfig {
            max_work: 100_000_000,
            use_summaries: false,
            root_terminal_coverage: false,
            ..Default::default()
        },
        HybridConfig::default(),
    );
    assert!(report.coverage().baseline_current && report.coverage().detail_current);
    assert!(!report.baseline().config().use_summaries);
    assert!(!report.detail().unwrap().config().use_summaries);
    assert!(
        report
            .baseline()
            .procedure_summaries()
            .iter()
            .any(|record| record
                .procedures
                .iter()
                .any(|p| p.semantics == TransferSemantics::KnownProcess))
    );
    assert!(report.local_detail().next().is_some());
    assert_eq!(report.coverage().chosen, HybridInterpretation::Native);
    assert_accounting(&report, 100_000_000);
}

#[test]
fn native_run_remains_available_without_process_policy_or_baseline_cost() {
    let policies = EnginePolicies::default();
    assert_eq!(<evm_abstract::analysis::SemanticPolicies as evm_abstract::analysis::ProcedureSummaryPolicy<evm_abstract::domain::ProductAlgebra>>::runtime_execution(&policies.state), RuntimeExecution::Native);
    let mut extension = NoExtension;
    let native = AnalysisEngine::new(&native::product(), &policies, &mut extension)
        .run(
            AnalysisInput::new(branch_world(), unknown()).unwrap(),
            ExecutionConfig {
                max_work: 50_000_000,
                root_terminal_coverage: false,
                ..Default::default()
            },
            &Control::default(),
        )
        .unwrap();
    assert_eq!(native.status(), Status::Converged);
    assert!(native.procedure_summaries().is_empty());
    assert!(native.states().iter().all(|s| s.active().process.is_none()));
}
