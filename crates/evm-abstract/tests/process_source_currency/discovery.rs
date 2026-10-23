use crate::{
    A, Assembly, address, assert_current, config, entry, evidence, fixture, insert, native,
    new_world, policies, root_epoch,
};
use alloy_primitives::keccak256;
use evm_abstract::{
    Fork,
    analysis::{
        AnalysisEngine, AnalysisInput, FrontierReason, ProcessSource, Status,
        control::Control,
        cooperation::NoExtension,
        layers::{
            AnalysisSession, CodeObservation, FactDemand, Observation, RefinementEffect,
            SessionError, SnapshotId,
        },
    },
    bytecode::Program,
    world::{EvmAccount, EvmByteArray},
};
use revm_bytecode::opcode;

#[test]
fn dynamic_target_missing_named_code_demands_then_restarts_with_a_new_epoch() {
    let mut code = Assembly::default();
    code.call_arguments(opcode::CALL, 0, (0, 0), (0, 0))
        .input(0)
        .push(60_000)
        .op(opcode::CALL)
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &code.finish(), &[]);
    let missing = address(0x202);
    world.insert(missing, EvmAccount::unknown()).unwrap();
    let values = native::product();
    let policies = policies();
    let mut extension = NoExtension;
    let mut session = AnalysisSession::new(
        AnalysisEngine::new(&values, &policies, &mut extension),
        AnalysisInput::new(world, entry(EvmByteArray::unknown_checked().unwrap())).unwrap(),
        config(),
    )
    .unwrap();
    let control = Control::default();
    let snapshot = session.facts().snapshot().clone();
    let old = session.run_with_demands(&control).unwrap();
    evidence("missing-before-promotion", old.analysis());
    assert_eq!(old.analysis().status(), Status::Incomplete);
    assert!(old.demands().contains(&FactDemand::Code(missing)));
    assert!(
        old.analysis()
            .frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::MissingCode(missing))
    );
    assert!(
        old.analysis()
            .procedure_summaries()
            .iter()
            .all(|r| !r.source_current)
    );
    let old_epoch = root_epoch(old.analysis());
    let old_json = serde_json::to_value(old.analysis().procedure_summaries()).unwrap();
    let replacement = Program::decode_with_fork(&[opcode::STOP], Fork::Osaka).unwrap();
    let id = session.supply_runtime(replacement.clone()).unwrap();
    let effect = session
        .refine(
            &snapshot,
            [Observation::Code {
                address: missing,
                code: CodeObservation::Runtime(id),
            }],
            &control,
        )
        .unwrap();
    assert!(
        matches!(effect, RefinementEffect::RestartRequired { executed_revision, .. }
        if executed_revision == old.revision())
    );
    let next = session.run_with_demands(&control).unwrap();
    evidence("missing-after-promotion", next.analysis());
    assert_ne!(next.revision(), old.revision());
    assert!(next.demands().is_empty());
    assert_current(next.analysis());
    assert_ne!(root_epoch(next.analysis()), old_epoch);
    assert!(next.analysis().procedure_summaries().iter().flat_map(|r| &r.procedures).any(|p|
        matches!(p.frame.process.as_ref().unwrap().source, ProcessSource::Runtime { source, hash, fork }
            if source == missing && hash == keccak256(replacement.bytes()) && fork == Fork::Osaka)));
    assert_eq!(
        serde_json::to_value(old.analysis().procedure_summaries()).unwrap(),
        old_json
    );
    assert!(old.demands().contains(&FactDemand::Code(missing)));
}

#[test]
fn captured_code_conflict_mixed_fork_and_wrong_snapshot_cannot_recertify() {
    let mut world = new_world();
    insert(&mut world, A, &fixture(&[opcode::STOP]), &[]);
    let values = native::product();
    let policies = policies();
    let mut extension = NoExtension;
    let mut session = AnalysisSession::new(
        AnalysisEngine::new(&values, &policies, &mut extension),
        AnalysisInput::new(world, entry(EvmByteArray::empty())).unwrap(),
        config(),
    )
    .unwrap();
    let control = Control::default();
    let before = session.run(&control).unwrap();
    assert_current(before.analysis());
    let revision = session.facts().revision();
    let snapshot = session.facts().snapshot().clone();
    let changed = session
        .supply_runtime(Program::decode_with_fork(&[opcode::INVALID], Fork::Osaka).unwrap())
        .unwrap();
    assert!(matches!(
        session.refine(
            &snapshot,
            [Observation::Code {
                address: address(A),
                code: CodeObservation::Runtime(changed)
            }],
            &control
        ),
        Err(SessionError::Facts(_)) | Err(SessionError::EntryCode(_))
    ));
    assert!(matches!(
        session.supply_runtime(Program::decode_with_fork(&[opcode::STOP], Fork::Cancun).unwrap()),
        Err(SessionError::Catalog(_))
    ));
    assert!(matches!(
        session.refine(
            &SnapshotId::offline("wrong snapshot"),
            [Observation::Code {
                address: address(0x202),
                code: CodeObservation::Empty
            }],
            &control
        ),
        Err(SessionError::Facts(_))
    ));
    assert_eq!(session.facts().revision(), revision);
    let after = session.run(&control).unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &after));
    assert_current(after.analysis());
}

#[test]
fn precise_outside_discovery_restarts_a_closed_complement_with_a_new_epoch() {
    let mut code = Assembly::default();
    code.call(opcode::STATICCALL, 0x202, 0, (0, 0), (0, 0))
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &code.finish(), &[]);
    let outside = address(0x202);
    let values = native::product();
    let policies = policies();
    let mut extension = NoExtension;
    let mut session = AnalysisSession::new(
        AnalysisEngine::new(&values, &policies, &mut extension),
        AnalysisInput::new(world, entry(EvmByteArray::empty())).unwrap(),
        config(),
    )
    .unwrap();
    let control = Control::default();
    let old = session.run_with_demands(&control).unwrap();
    assert_current(old.analysis());
    assert!(
        old.demands().is_empty(),
        "an unresolved target set is not an enumerable code demand"
    );
    assert!(old.analysis().procedure_summaries().iter().flat_map(|r| &r.procedures).all(|p|
        !matches!(p.frame.process.as_ref().unwrap().source, ProcessSource::Runtime { source, .. } if source == outside)));
    let old_epoch = root_epoch(old.analysis());
    let old_json = serde_json::to_value(old.analysis().procedure_summaries()).unwrap();
    let snapshot = session.facts().snapshot().clone();
    let id = session
        .supply_runtime(Program::decode_with_fork(&[opcode::STOP], Fork::Osaka).unwrap())
        .unwrap();
    let effect = session
        .refine(
            &snapshot,
            [Observation::Code {
                address: outside,
                code: CodeObservation::Runtime(id),
            }],
            &control,
        )
        .unwrap();
    assert!(
        matches!(effect, RefinementEffect::RestartRequired { executed_revision, ref affected_states, .. }
        if executed_revision == old.revision() && affected_states.contains(&0))
    );
    let new = session.run_with_demands(&control).unwrap();
    evidence("outside-precise-promotion", new.analysis());
    assert_current(new.analysis());
    assert_ne!(new.revision(), old.revision());
    assert_ne!(root_epoch(new.analysis()), old_epoch);
    assert!(new.analysis().procedure_summaries().iter().flat_map(|r| &r.procedures).any(|p|
        matches!(p.frame.process.as_ref().unwrap().source, ProcessSource::Runtime { source, .. } if source == outside)));
    // The old certificate is still true for its old snapshot revision; it is
    // neither edited in place nor installed as the new revision's certificate.
    assert_eq!(
        serde_json::to_value(old.analysis().procedure_summaries()).unwrap(),
        old_json
    );
    assert_current(old.analysis());
}

#[test]
fn missing_delegation_implementation_is_promoted_under_its_actual_source_hash() {
    let mut code = Assembly::default();
    code.call(opcode::STATICCALL, 0x202, 0, (0, 0), (0, 0))
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &code.finish(), &[]);
    let pointer = address(0x202);
    let implementation = address(0x303);
    let mut account = EvmAccount::unknown();
    account.code = evm_abstract::world::Code::Delegation(implementation);
    world.insert(pointer, account).unwrap();
    let values = native::product();
    let policies = policies();
    let mut extension = NoExtension;
    let mut session = AnalysisSession::new(
        AnalysisEngine::new(&values, &policies, &mut extension),
        AnalysisInput::new(world, entry(EvmByteArray::empty())).unwrap(),
        config(),
    )
    .unwrap();
    let control = Control::default();
    let old = session.run_with_demands(&control).unwrap();
    assert_eq!(old.analysis().status(), Status::Incomplete);
    assert!(old.demands().contains(&FactDemand::Code(implementation)));
    let snapshot = session.facts().snapshot().clone();
    let id = session
        .supply_runtime(Program::decode_with_fork(&[opcode::STOP], Fork::Osaka).unwrap())
        .unwrap();
    session
        .refine(
            &snapshot,
            [Observation::Code {
                address: implementation,
                code: CodeObservation::Runtime(id),
            }],
            &control,
        )
        .unwrap();
    let next = session.run_with_demands(&control).unwrap();
    assert_current(next.analysis());
    assert!(next.demands().is_empty());
    assert_ne!(root_epoch(next.analysis()), root_epoch(old.analysis()));
    assert!(
        next.analysis()
            .procedure_summaries()
            .iter()
            .flat_map(|r| &r.procedures)
            .any(|p| {
                let frame = p.frame.process.as_ref().unwrap();
                frame.owner == evm_abstract::analysis::ProcessOwner::Named(pointer)
                    && frame.source
                        == ProcessSource::Runtime {
                            source: implementation,
                            hash: keccak256([opcode::STOP]),
                            fork: Fork::Osaka,
                        }
            })
    );
}
