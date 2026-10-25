//! Coherent revm effects and bounded joint-source regression controls.
#[path = "support/assembly.rs"]
mod assembly;
#[path = "support/revm.rs"]
mod concrete;
#[path = "support/correlation.rs"]
mod correlation;
#[path = "support/native.rs"]
mod native;

use alloy_primitives::{Address, U256};
use evm_abstract::{
    Fork,
    analysis::{AnalysisInput, ExecutionConfig, Status, control::Control},
    domain::{
        EvmValue, NumericValue,
        symbolic::{ExprId, ExprLimits},
    },
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use revm_bytecode::opcode;

fn address(n: u64) -> Address {
    Address::from_word(U256::from(n).into())
}

fn input_world(code: &[u8], slot: Option<u64>) -> EvmWorld<NumericValue> {
    let mut world = EvmWorld::new(Fork::Osaka, "oracle:joint-assignment");
    let mut account =
        EvmAccount::from_hex(&alloy_primitives::hex::encode(code), Fork::Osaka).unwrap();
    account.balance = EvmValue::constant(U256::from(1000));
    account.storage_unknown = slot.is_none();
    if let Some(slot) = slot {
        account
            .storage
            .insert(U256::ZERO, EvmValue::constant(U256::from(slot)));
    }
    world.insert(address(0x101), account).unwrap();
    world
}

#[test]
fn one_assignment_covers_return_and_storage_and_rejects_crossed_components() {
    let mut code = assembly::Assembly::default();
    // old S[0] = x; return (x,x+1) and store S[1]=x, S[2]=x+1.
    code.push(0)
        .op(opcode::SLOAD)
        .op(opcode::DUP1)
        .push(1)
        .op(opcode::SSTORE)
        .op(opcode::DUP1)
        .push(0)
        .op(opcode::MSTORE)
        .push(1)
        .op(opcode::ADD)
        .op(opcode::DUP1)
        .push(2)
        .op(opcode::SSTORE)
        .push(32)
        .op(opcode::MSTORE)
        .push(64)
        .push(0)
        .op(opcode::RETURN);
    let code = code.finish().bytes;
    let entry = EvmEntry::concrete(
        address(0x101),
        address(0x900),
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let world = input_world(&code, None);
    let graph = native::run(
        &native::product(),
        AnalysisInput::new(world, entry.clone()).unwrap(),
        ExecutionConfig {
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .unwrap();
    assert_eq!(graph.status(), Status::Converged);
    let outcome = graph
        .outcomes()
        .iter()
        .find(|outcome| outcome.data.len().singleton() == Some(U256::from(64)))
        .unwrap();
    let read = |slot| {
        outcome.store.read(
            address(0x101),
            &EvmValue::constant(U256::from(slot)),
            native::product().borrowed(),
        )
    };
    let x = read(1).symbolic_expression().expect("retained x source");
    let y = read(2)
        .symbolic_expression()
        .expect("retained x+1 expression");
    let output_x = outcome
        .data
        .read_word(
            &EvmValue::constant(U256::ZERO),
            native::product().borrowed(),
        )
        .symbolic_expression()
        .unwrap();
    let output_y = outcome
        .data
        .read_word(
            &EvmValue::constant(U256::from(32)),
            native::product().borrowed(),
        )
        .symbolic_expression()
        .unwrap();
    let inputs = [(x.clone(), vec![U256::ONE, U256::from(2)])];
    for n in [1, 2] {
        let observed = concrete::execute(&input_world(&code, Some(n)), &entry);
        observed.covers(&graph);
        let tuple = [
            (x.clone(), observed.slot(0x101, 1)),
            (y.clone(), observed.slot(0x101, 2)),
            (output_x.clone(), observed.output_word(0)),
            (output_y.clone(), observed.output_word(32)),
        ];
        assert_eq!(correlation::includes(&inputs, &tuple, &[]), Ok(true));
    }
    // Every marginal contains its value; no common x yields (1,3).
    assert!(read(1).contains(U256::ONE));
    assert!(read(2).contains(U256::from(3)));
    assert_eq!(
        correlation::includes(
            &inputs,
            &[(x.clone(), U256::ONE), (y.clone(), U256::from(3))],
            &[]
        ),
        Ok(false)
    );
    let is_two = ExprId::operation(
        opcode::EQ,
        &[x.clone(), ExprId::constant(U256::from(2))],
        ExprLimits::default(),
    )
    .unwrap();
    assert_eq!(
        correlation::includes(&inputs, &[(y, U256::from(2))], &[(is_two, true)]),
        Ok(false)
    );
}

#[test]
fn joint_checker_reports_unsupported_unassigned_and_oversized_domains() {
    let x = ExprId::fresh().unwrap();
    let unsupported =
        ExprId::operation(opcode::CLZ, std::slice::from_ref(&x), ExprLimits::default()).unwrap();
    let inputs = [(x.clone(), vec![U256::ONE])];
    assert_eq!(
        correlation::includes(&inputs, &[(unsupported, U256::ZERO)], &[]),
        Err(correlation::WitnessError::Unsupported)
    );
    assert_eq!(
        correlation::includes(&[], &[(x.clone(), U256::ZERO)], &[]),
        Err(correlation::WitnessError::Unassigned)
    );
    assert_eq!(
        correlation::includes(&[(x, vec![U256::ZERO; 65])], &[], &[]),
        Err(correlation::WitnessError::Bound)
    );
}

#[test]
fn ordered_concrete_logs_survive_but_may_inclusion_does_not_claim_order_or_count() {
    for revert in [false, true] {
        let mut code = assembly::Assembly::default();
        for topic in [11, 22] {
            code.push(topic).push(0).push(0).op(opcode::LOG1);
        }
        if revert {
            code.push(0).push(0).op(opcode::REVERT);
        } else {
            code.op(opcode::STOP);
        }
        let world = input_world(&code.finish().bytes, Some(0));
        let entry = EvmEntry::concrete(
            address(0x101),
            address(0x900),
            EvmValue::constant(U256::ZERO),
            EvmByteArray::empty(),
        );
        let observed = concrete::execute(&world, &entry);
        assert_eq!(
            observed
                .emitted_logs
                .iter()
                .map(|(_, topics, _)| topics[0])
                .collect::<Vec<_>>(),
            [U256::from(11), U256::from(22)]
        );
        let graph = native::run(
            &native::product(),
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig {
                root_terminal_coverage: false,
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
        .unwrap();
        observed.covers(&graph);
        if revert {
            assert!(observed.logs.is_empty());
        } else {
            assert_eq!(observed.logs, observed.emitted_logs);
            let mut reordered = observed.clone();
            reordered.logs.reverse();
            assert!(
                reordered.is_covered(&graph),
                "may-log representation has no sequence"
            );
            reordered.logs.push(observed.logs[0].clone());
            assert!(
                reordered.is_covered(&graph),
                "may-log representation has no multiplicity"
            );
            reordered.logs[0].1[0] = U256::from(99);
            assert!(
                !reordered.is_covered(&graph),
                "unsupported log contents must be rejected"
            );
        }
    }
}
