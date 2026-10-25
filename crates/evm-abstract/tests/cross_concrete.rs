//! Offline multi-account revm differentials. The oracle loads the same bytes
//! and initial state, but does not reuse the abstract interpreter's transfers.
//! Finite fixtures establish regression evidence, not an EVM-wide proof.
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;

#[path = "support/native.rs"]
mod native;

use alloy_primitives::Address;
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork, U256,
    analysis::{
        EvmWorldAnalysis, ExecutionConfig, FrameCode, MachineEdgeKind, OutcomeKind,
        ProcedureSummaryKind, Status,
    },
    domain::EvmValue,
    ssa,
    world::{Code, EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use std::collections::BTreeMap;
#[path = "support/revm.rs"]
mod concrete;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn fixture(name: &str, fork: Fork) -> EvmWorld<NumericValue> {
    let text = std::fs::read_to_string(format!(
        "{}/../../examples/worlds/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut world = EvmWorld::<NumericValue>::new(fork, json["provenance"].as_str().unwrap());
    for input in json["accounts"].as_array().unwrap() {
        let address = input["address"].as_str().unwrap().parse().unwrap();
        let code = input["code"].as_str().unwrap();
        let balance: U256 = input["balance"].as_str().unwrap().parse().unwrap();
        let mut account = EvmAccount::<NumericValue>::from_hex(code, fork).unwrap();
        account.balance = EvmValue::<NumericValue>::constant(balance);
        assert_eq!(input["storage_unknown"], false);
        for (slot, value) in input["storage"].as_object().unwrap() {
            let slot: U256 = slot.parse().unwrap();
            let value: U256 = value.as_str().unwrap().parse().unwrap();
            account
                .storage
                .insert(slot, EvmValue::<NumericValue>::constant(value));
        }
        world.insert(address, account).unwrap();
    }
    world
}

fn covers_steps(analysis: &EvmWorldAnalysis<NumericValue>, steps: &[concrete::Step]) {
    for step in steps {
        let matching: Vec<_> = analysis
            .states()
            .iter()
            .filter(|state| {
                let frame = state.active();
                frame.code_address == step.code
                    && frame.address == step.owner
                    && frame.caller.as_concrete() == Some(step.caller)
                    && frame.is_static == step.is_static
                    && state.executed_pcs.contains(&step.pc)
            })
            .collect();
        // A recursive callee's actual analyzed blocks belong to the procedure
        // record. Its public evidence does not fabricate ordinary machine nodes.
        // Every available main-graph block-entry stack is still checked below;
        // compare() separately requires one outcome to cover all concrete effects.
        let in_procedure = analysis.procedure_summaries().iter().any(|record| {
            record.kind == ProcedureSummaryKind::KnownRecursion
                && record.closed
                && record
                    .root_procedure
                    .and_then(|root| record.procedures.get(root))
                    .is_some_and(|root| !root.outputs.is_empty())
                && record.procedures.iter().any(|procedure| {
                    let frame = &procedure.frame;
                    let Some(account) = analysis.world().account(step.code) else {
                        return false;
                    };
                    let Code::Runtime(program) = &account.code else {
                        return false;
                    };
                    frame.mode == FrameCode::Runtime
                        && frame.code_hash == alloy_primitives::keccak256(program.bytes())
                        && frame.code_address == step.code
                        && frame.address == step.owner
                        && frame.caller.as_concrete() == Some(step.caller)
                        && frame.is_static == step.is_static
                        && !procedure.outputs.is_empty()
                        && procedure.covered_blocks.iter().any(|block| {
                            program.blocks().get(*block).is_some_and(|block| {
                                block.instructions.iter().any(|i| i.pc == step.pc)
                            })
                        })
                })
        });
        assert!(
            !matching.is_empty() || in_procedure,
            "concrete opcode/frame missing from analyzed graph and closed procedure: {step:?}"
        );
        if matching
            .iter()
            .any(|state| state.executed_pcs.first() == Some(&step.pc))
        {
            assert!(
                matching.iter().any(|state| {
                    let entry = &state.entry.active().stack;
                    entry.len() == step.stack.len()
                        && entry
                            .iter()
                            .zip(&step.stack)
                            .all(|(value, concrete)| value.contains(*concrete))
                }),
                "concrete block-entry stack not covered: {step:?}"
            );
        }
    }
}

fn compare(name: &str, fork: Fork) -> EvmWorldAnalysis<NumericValue> {
    let world = fixture(name, fork);
    let entry = EvmEntry {
        address: address(0x101),
        environment: evm_abstract::world::EvmEnvironmentValues {
            to: (address(0x101)).into(),
            caller: (address(0x1000)).into(),
            value: EvmValue::<NumericValue>::constant(U256::ZERO),
            calldata: EvmByteArray::<NumericValue>::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironmentValues::<NumericValue>::default()
        },
    };
    let analysis = native::run(
        &native::product(),
        AnalysisInput::new(world.clone(), entry.clone()).unwrap(),
        ExecutionConfig::default(),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(analysis.status(), Status::Converged, "{name} {fork}");
    ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
    let observed = concrete::execute(&world, &entry);
    assert_eq!(observed.kind, OutcomeKind::Return, "{name} {fork}");
    assert!(
        !observed.steps.is_empty(),
        "oracle did not execute fixture runtime"
    );
    assert!(
        observed
            .steps
            .iter()
            .any(|step| step.code != address(0x101)),
        "oracle did not enter any fixture callee"
    );
    covers_steps(&analysis, &observed.steps);
    observed.covers(&analysis);
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Call)
    );
    analysis
}

fn outcome_slots(
    analysis: &EvmWorldAnalysis<NumericValue>,
) -> BTreeMap<(Address, U256), EvmValue<NumericValue>> {
    let mut stores = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .map(|outcome| &outcome.store);
    let first = stores.next().unwrap().clone();
    stores
        .fold(first, |joined, store| {
            joined.join(store, native::product().borrowed())
        })
        .slots()
        .iter()
        .map(|(key, value)| (*key, value.clone()))
        .collect()
}

fn finite_contains(value: &EvmValue<NumericValue>, expected: u64) {
    assert!(
        value.constants().is_some(),
        "expected a finite effect, got {value}"
    );
    assert!(
        value.contains(U256::from(expected)),
        "{value} misses {expected}"
    );
}

#[test]
fn calls_returns_and_account_effects_cover_revm_under_each_supported_fork() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for name in [
            "call-return-branch",
            "proxy-storage",
            "callcode-context",
            "returndata-copy",
            "revert-rollback",
            "static-write",
            "reentry",
            "log-rollback",
        ] {
            let analysis = compare(name, fork);
            match name {
                "call-return-branch" => returned_word_controls_the_caller_branch(&analysis),
                "proxy-storage" => {
                    shared_implementation_preserves_proxy_storage_caller_and_callvalue(&analysis)
                }
                "callcode-context" => callcode_uses_proxy_caller_and_explicit_callvalue(&analysis),
                "revert-rollback" => revert_restores_callee_state(&analysis),
                "static-write" => static_failure_restores_callee_state(&analysis),
                "reentry" => reentrant_read_observes_the_current_transaction_store(&analysis),
                "log-rollback" => reverted_child_logs_do_not_leak_into_returned_store(&analysis),
                "returndata-copy" => {}
                _ => unreachable!(),
            }
        }
    }
}

fn returned_word_controls_the_caller_branch(analysis: &EvmWorldAnalysis<NumericValue>) {
    finite_contains(&outcome_slots(analysis)[&(address(0x101), U256::ZERO)], 1);
}

fn shared_implementation_preserves_proxy_storage_caller_and_callvalue(
    analysis: &EvmWorldAnalysis<NumericValue>,
) {
    let slots = outcome_slots(analysis);
    for (proxy, counter, callvalue) in [(0x201, 6, 7), (0x202, 10, 11)] {
        for (slot, expected) in [(0, counter), (1, proxy), (2, 0x101), (3, callvalue)] {
            finite_contains(&slots[&(address(proxy), U256::from(slot))], expected);
        }
    }
    assert_eq!(
        slots[&(address(0x300), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(99))
    );
}

fn callcode_uses_proxy_caller_and_explicit_callvalue(analysis: &EvmWorldAnalysis<NumericValue>) {
    let slots = outcome_slots(analysis);
    for proxy in [0x201, 0x202] {
        finite_contains(&slots[&(address(proxy), U256::from(2))], proxy);
        finite_contains(&slots[&(address(proxy), U256::from(3))], 3);
    }
}

fn revert_restores_callee_state(analysis: &EvmWorldAnalysis<NumericValue>) {
    let slots = outcome_slots(analysis);
    assert_eq!(
        slots[&(address(0x101), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(3))
    );
    finite_contains(&slots[&(address(0x101), U256::from(1))], 42);
    assert_eq!(
        slots[&(address(0x101), U256::from(2))],
        EvmValue::<NumericValue>::constant(U256::ZERO)
    );
    assert_eq!(
        slots[&(address(0x200), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(4))
    );
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Revert)
    );
}

fn static_failure_restores_callee_state(analysis: &EvmWorldAnalysis<NumericValue>) {
    let slots = outcome_slots(analysis);
    assert_eq!(
        slots[&(address(0x101), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::ZERO)
    );
    assert_eq!(
        slots[&(address(0x200), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(4))
    );
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Failure)
    );
}

fn reentrant_read_observes_the_current_transaction_store(
    analysis: &EvmWorldAnalysis<NumericValue>,
) {
    let slots = outcome_slots(analysis);
    assert_eq!(
        slots[&(address(0x101), U256::ZERO)],
        EvmValue::<NumericValue>::constant(U256::from(2))
    );
    finite_contains(&slots[&(address(0x101), U256::from(1))], 1);
    assert!(
        analysis
            .states()
            .iter()
            .any(|state| state.key.frames.len() == 3)
    );
}

fn reverted_child_logs_do_not_leak_into_returned_store(analysis: &EvmWorldAnalysis<NumericValue>) {
    for outcome in analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
    {
        assert!(!outcome.store.logs_unknown());
        assert!(
            outcome
                .store
                .possible_logs()
                .keys()
                .all(|site| site.address == address(0x101))
        );
    }
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return
                && !outcome.store.possible_logs().is_empty())
    );
}
