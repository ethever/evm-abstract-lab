//! Code-only acceptance: no observed caller, value, calldata or external world.
use evm_abstract::analysis::AnalysisInput;
use evm_abstract::analysis::control::Control;
#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Address, Fork, U256,
    analysis::{ExecutionConfig, OutcomeKind, Status},
    domain::symbolic::ByteStateKind,
    world::{Code, EvmAccount, EvmEntry, EvmWorld},
};

#[test]
fn code_only_memory_copy_returns_the_complete_original_input_at_default_work_budget() {
    let input: serde_json::Value = serde_json::from_str(include_str!(
        "../../../examples/no-smt-scale/code-only/memory-copy.json"
    ))
    .unwrap();
    let observed = &input["accounts"][0];
    let owner: Address = observed["address"].as_str().unwrap().parse().unwrap();
    let code = observed["code"].as_str().unwrap();
    assert_eq!(code, "0x365f6020373660205f5e365ff3");
    let mut world = EvmWorld::<NumericValue>::offline(
        Fork::Osaka,
        input["identity"]["label"].as_str().unwrap(),
        input["provenance"].as_str().unwrap(),
    );
    world
        .insert(
            owner,
            EvmAccount {
                code: Code::Runtime(
                    evm_abstract::bytecode::Program::from_hex_with_fork(code, Fork::Osaka).unwrap(),
                ),
                ..EvmAccount::<NumericValue>::unknown()
            },
        )
        .unwrap();
    let entry: EvmEntry<NumericValue> = EvmEntry {
        address: owner,
        environment: Default::default(),
    };
    assert!(entry.environment.caller.as_concrete().is_none());
    assert!(entry.environment.to.as_concrete().is_none());
    assert!(entry.environment.value.numeric().is_top());
    assert!(entry.environment.calldata.len().numeric().is_top());
    let config = ExecutionConfig::default();
    assert_eq!(config.max_work, 20_000_000);
    let graph = native::run(
        &native::product(),
        AnalysisInput::new(world, entry).unwrap(),
        config,
        &Control::default(),
    )
    .unwrap();
    assert_eq!(graph.status(), Status::Converged, "{:?}", graph.frontiers());
    assert!(graph.frontiers().is_empty());
    let original = &graph.states()[0].entry.active().calldata;
    let mut successes = 0;
    for outcome in graph
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
    {
        successes += 1;
        assert_eq!(graph.states()[outcome.state].executed_pcs.last(), Some(&12));
        assert_eq!(outcome.data.len().expression(), original.len().expression());
        for length in [U256::ZERO, U256::from(1), U256::from(1) << 128usize] {
            assert!(
                outcome.data.len().contains(length),
                "L is not clamped to a host limit"
            );
        }
        let returned = outcome.data.view().canonicalize();
        if returned.state == *original.state_root() {
            assert_eq!(returned.offset.as_constant(), Some(U256::ZERO));
            assert_eq!(&returned.length, original.len().expression().unwrap());
        } else {
            // MCOPY's complete range at zero is canonicalized to its captured
            // source, the CALLDATACOPY range at 32. The success guard supplies
            // nonwrapping arithmetic; no unrelated scalar bound is injected.
            assert_eq!(returned.offset.as_constant(), Some(U256::from(32)));
            let ByteStateKind::Copy {
                destination,
                source,
                length,
                ..
            } = returned.state.kind()
            else {
                panic!("return must retain the full captured copy: {returned:?}");
            };
            assert_eq!(destination.as_constant(), Some(U256::from(32)));
            assert_eq!(&returned.length, length);
            assert_eq!(&source.length, length);
            assert_eq!(source.offset.as_constant(), Some(U256::ZERO));
            assert_eq!(&source.state, original.state_root());
            assert_eq!(length, original.len().expression().unwrap());
        }
    }
    assert!(
        successes > 0,
        "reach pc 12 RETURN with length L (pc 11 is PUSH0)"
    );
    assert!(
        graph
            .outcomes()
            .iter()
            .filter(
                |o| o.kind == OutcomeKind::Failure && o.data.len().singleton() == Some(U256::ZERO)
            )
            .count()
            >= 3
    );
}
