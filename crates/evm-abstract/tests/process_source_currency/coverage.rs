use crate::{A, Assembly, address, assert_current, fixture, insert, oracle, run, support};
use evm_abstract::{
    Fork,
    analysis::{OutcomeKind, ProcessAction, ProcessLocation},
    bytecode::{DecodeError, Program},
    world::{EvmAccount, EvmByteArray, EvmWorld},
};
use revm_bytecode::opcode;

#[test]
fn every_valid_opcode_has_a_current_primitive_or_collapsed_process_rule() {
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let mut valid = 0;
        for op in 0..=u8::MAX {
            let mut bytes = vec![op];
            if (opcode::PUSH1..=opcode::PUSH32).contains(&op) {
                bytes.resize(1 + usize::from(op - opcode::PUSH1 + 1), 0);
            }
            bytes.push(opcode::STOP);
            let decoded = match Program::decode_with_fork(&bytes, fork) {
                Ok(program) => program,
                Err(DecodeError::UnsupportedEof) => {
                    // EF00 is a rejected container prefix, not a valid legacy
                    // opcode silently omitted from the process inventory.
                    assert_eq!(op, 0xef);
                    continue;
                }
                Err(error) => panic!("opcode={op:02x}, fork={fork:?}: {error}"),
            };
            let instruction = &decoded.blocks()[0].instructions[0];
            if !instruction.is_valid() {
                continue;
            }
            valid += 1;
            let expected = match op {
                opcode::CALL => vec![ProcessAction::Call],
                opcode::STATICCALL => vec![ProcessAction::StaticCall],
                opcode::CALLCODE => vec![ProcessAction::CallCode],
                opcode::DELEGATECALL => vec![ProcessAction::DelegateCall],
                opcode::CREATE => vec![ProcessAction::Create],
                opcode::CREATE2 => vec![ProcessAction::Create2],
                _ => Vec::new(),
            };
            let mut world = EvmWorld::new(fork, "process-opcode-inventory");
            insert(&mut world, A, &fixture(&bytes), &[]);
            let analysis = run(world);
            assert_current(&analysis);
            let record = analysis.procedure_summaries().last().unwrap();
            let root = record.root_procedure.unwrap();
            let mut entry_seen = false;
            for node in record
                .regional_cfg
                .nodes
                .iter()
                .filter(|node| node.procedure == root)
            {
                let receipt = node.transfer.as_ref().unwrap().process.as_ref().unwrap();
                let entry =
                    node.frame.process.as_ref().unwrap().location == ProcessLocation::Block(0);
                entry_seen |= entry;
                // The original fixture and complete opcode/fork inventory stay
                // intact. Its primitive belongs to the entry block; STOP/EOF
                // continuations must not repeat a whole-source action bag.
                assert_eq!(
                    receipt.actions,
                    if entry { expected.as_slice() } else { &[] },
                    "opcode={op:02x}, fork={fork:?}, location={:?}",
                    node.frame.process.as_ref().unwrap().location
                );
                assert!(receipt.unresolved.is_empty());
            }
            assert!(
                entry_seen,
                "missing admitted opcode entry: opcode={op:02x}, fork={fork:?}"
            );
        }
        assert!(valid > 140);
        eprintln!("process opcode inventory: fork={fork:?}, valid={valid}");
    }
}

#[test]
fn collapsed_local_effect_rules_cover_independent_evm_observations() {
    let mut storage = Assembly::default();
    storage.store(0, 9).op(opcode::STOP);
    let mut transient = Assembly::default();
    transient
        .push(9)
        .push(0)
        .op(opcode::TSTORE)
        .push(0)
        .op(opcode::TLOAD)
        .mstore(0)
        .return_range(0, 32);
    let mut log = Assembly::default();
    log.push(9)
        .mstore(0)
        .push(32)
        .push(0)
        .op(opcode::LOG0)
        .op(opcode::STOP);
    let mut destruction = Assembly::default();
    destruction.push(0x202).op(opcode::SELFDESTRUCT);
    let mut revert = Assembly::default();
    revert.store(0, 9).push(0).push(0).op(opcode::REVERT);
    for code in [
        storage.finish(),
        transient.finish(),
        log.finish(),
        destruction.finish(),
        revert.finish(),
        fixture(&[opcode::INVALID]),
    ] {
        let mut world = support::new_world();
        insert(&mut world, A, &code, &[(0, 7)]);
        world.insert(address(0x202), EvmAccount::empty()).unwrap();
        let observed = oracle::execute(&world, &support::entry(EvmByteArray::empty()));
        let analysis = run(world);
        assert_current(&analysis);
        observed.covers(&analysis);
        // An actual entered Failure must also have a regional receipt. The
        // engine's independent pre-entry gas Failure cannot satisfy this check.
        assert!(
            analysis
                .procedure_summaries()
                .iter()
                .flat_map(|r| &r.regional_cfg.nodes)
                .filter_map(|n| n.transfer.as_ref())
                .any(|r| r.outcomes.contains(&OutcomeKind::Failure))
        );
    }
}
