use super::*;
use evm_decompile::{EntityRef, LiftedFailure};
use std::collections::BTreeSet;

fn rejected(stage: &Stage, name: &str) {
    let result = validate_stage(stage);
    assert_eq!(
        result.status,
        ValidationStatus::Failed,
        "corrupt {name} accepted: {result:?}"
    );
}

#[test]
fn corrupt_failure_ranks_edges_roots_polarity_and_pass_plan_fail_certification() {
    let artifact = checked(&direct(false, Failure::Diamond));
    let stage = lifted(&artifact).0;
    let position = lifted(&artifact)
        .1
        .controls
        .iter()
        .position(|c| c.guard.is_some())
        .unwrap();
    let mut cases = Vec::new();
    let mut wrong = stage.clone();
    wrong
        .ir
        .lifted
        .as_mut()
        .unwrap()
        .failures
        .iter_mut()
        .find(|fact| fact.rank > 0)
        .unwrap()
        .rank = 0;
    cases.push(("rank", wrong));
    let mut wrong = stage.clone();
    wrong
        .ir
        .lifted
        .as_mut()
        .unwrap()
        .failures
        .retain(|fact| fact.rank != 0);
    cases.push(("failure root", wrong));
    for field in [
        "polarity",
        "pass_edge",
        "failure_edge",
        "pass_node",
        "failure_node",
        "block",
    ] {
        let mut wrong = stage.clone();
        let guard = wrong.ir.lifted.as_mut().unwrap().controls[position]
            .guard
            .as_mut()
            .unwrap();
        match field {
            "polarity" => guard.pass_nonzero = !guard.pass_nonzero,
            "pass_edge" => guard.pass_edge = guard.failure_edge,
            "failure_edge" => guard.failure_edge = guard.pass_edge,
            "pass_node" => guard.pass_node = guard.failure_node,
            "failure_node" => guard.failure_node = guard.pass_node,
            "block" => {
                guard.block = stage
                    .ir
                    .edges
                    .iter()
                    .find(|edge| edge.id == guard.pass_edge)
                    .unwrap()
                    .to
            }
            _ => unreachable!(),
        }
        cases.push((field, wrong));
    }
    let mut wrong = stage.clone();
    let control = &mut wrong.ir.lifted.as_mut().unwrap().controls[position];
    control.condition = stage
        .ir
        .lifted
        .as_ref()
        .unwrap()
        .expressions
        .iter()
        .find(|expr| expr.ty == evm_decompile::LiftedType::Word256)
        .unwrap()
        .id;
    cases.push(("predicate", wrong));
    for (name, wrong) in cases {
        rejected(&wrong, name);
    }
}

#[test]
fn corrupt_continuation_range_scope_or_reachable_join_omission_is_rejected() {
    for input in [decoder(false), fixtures::join_revert()] {
        let artifact = checked(&input);
        let stage = lifted(&artifact).0;
        let position = lifted(&artifact)
            .1
            .controls
            .iter()
            .position(|c| c.guard.as_ref().is_some_and(|g| g.continuation.is_some()))
            .expect("real If-following-failure continuation fixture");
        for field in ["sequence", "start", "end", "fallthrough", "absent"] {
            let mut wrong = stage.clone();
            let guard = wrong.ir.lifted.as_mut().unwrap().controls[position]
                .guard
                .as_mut()
                .unwrap();
            let tail = guard.continuation.as_mut().unwrap();
            match field {
                "sequence" => tail.sequence = guard.pass_node,
                "start" => tail.start += 1,
                "end" => tail.end -= 1,
                "fallthrough" => tail.pass_falls_through = !tail.pass_falls_through,
                "absent" => guard.continuation = None,
                _ => unreachable!(),
            }
            rejected(&wrong, field);
        }
    }
}

#[test]
fn a_loop_cannot_be_certified_by_inventing_a_positive_or_zero_failure_rank() {
    let artifact = checked(&fixtures::loop_failure(true));
    let stage = lifted(&artifact).0;
    let loop_block = stage.ir.blocks.iter().find(|block| matches!(&block.terminator,
        Terminator::Jump { edges, .. } if edges.iter().any(|id| stage.ir.edge(*id).unwrap().to == block.id)))
        .or_else(|| stage.ir.blocks.iter().find(|block| matches!(block.terminator, Terminator::Branch { .. }) && block.id != stage.ir.entry.unwrap()))
        .unwrap();
    for rank in [0, 1] {
        let mut wrong = stage.clone();
        wrong
            .ir
            .lifted
            .as_mut()
            .unwrap()
            .failures
            .push(LiftedFailure {
                block: loop_block.id,
                rank,
                payload: evm_decompile::LiftedGuardPayload::EmptyRevert,
            });
        wrong
            .ir
            .lifted
            .as_mut()
            .unwrap()
            .failures
            .sort_by_key(|fact| fact.block);
        rejected(&wrong, "cyclic failure rank");
    }
}

#[test]
fn guard_identity_and_absorbed_failure_operations_remain_navigable_in_raw_history() {
    let artifact = checked(&decoder(false));
    let (stage, program) = lifted(&artifact);
    let prior = structured(&artifact);
    let prior_records = evm_decompile::records(&prior.ir);
    let current_records = evm_decompile::records(&stage.ir);
    for control in program.controls.iter().filter(|c| c.guard.is_some()) {
        let guard = control.guard.as_ref().unwrap();
        let occurrence = EntityRef {
            stage: stage.id,
            entity: control.id,
        };
        let origin = artifact.origin(occurrence).unwrap();
        assert!(!origin.source.is_empty() && !origin.parents.is_empty());
        for source in &origin.source {
            assert!(artifact.descendants(source.pc).contains(&occurrence));
        }
        assert!(
            guard.source_references().count() <= 6,
            "direct references must not expand into a transitive failure closure"
        );
        for id in std::iter::once(control.node).chain(guard.source_references()) {
            assert!(prior_records.contains_key(&id) && current_records.contains_key(&id));
            assert!(
                artifact
                    .origin(EntityRef {
                        stage: stage.id,
                        entity: id
                    })
                    .is_some()
            );
        }
        assert!(
            origin
                .parents
                .iter()
                .any(|parent| parent.entity == control.node)
        );
    }
    for failure in &program.failures {
        let block = stage.ir.block(failure.block).unwrap();
        for operation in &block.operations {
            let occurrence = EntityRef {
                stage: stage.id,
                entity: operation.id,
            };
            assert_eq!(
                Some(operation),
                prior
                    .ir
                    .blocks
                    .iter()
                    .flat_map(|block| &block.operations)
                    .find(|old| old.id == operation.id)
            );
            assert!(artifact.origin(occurrence).is_some());
            if let Some(span) = operation.source {
                assert!(artifact.descendants(span.pc).contains(&occurrence));
            }
            if operation.opcode == 0xfd {
                assert!(
                    program
                        .block(block.id)
                        .unwrap()
                        .statements
                        .iter()
                        .any(|statement| statement.operation == operation.id),
                    "hidden raw REVERT must remain inspectable in the high block witness"
                );
            }
        }
    }
    let diff = artifact.diff(prior.id, stage.id).unwrap();
    assert!(diff.removed.is_empty() && diff.changed.is_empty());
    assert_eq!(
        diff.added.into_iter().collect::<BTreeSet<_>>(),
        program.entity_ids().collect()
    );
    let restored: Artifact =
        serde_json::from_slice(&serde_json::to_vec(&artifact).unwrap()).unwrap();
    assert_eq!(restored, artifact);
    assert_unchanged_witness(&artifact);
}

#[test]
fn schema_three_guards_cannot_be_disguised_as_legacy_artifacts() {
    let artifact = checked(&direct(false, Failure::Empty(U256::ZERO)));
    assert_eq!(artifact.schema_version, 6);
    assert!(artifact.admits_schema());
    for version in [3, 4] {
        let mut legacy = artifact.clone();
        legacy.schema_version = version;
        assert!(legacy.admits_schema());
        let encoded = serde_json::to_string(&legacy).unwrap();
        assert_eq!(serde_json::from_str::<Artifact>(&encoded).unwrap(), legacy);
        assert!(!encoded.contains("\"payloads\"") && !encoded.contains("\"constructions\""));
    }
    for version in [1, 2, 6] {
        let mut old = artifact.clone();
        old.schema_version = version;
        assert!(
            !old.admits_schema(),
            "guard-bearing schema {version} accepted"
        );
    }
    let mut table_only = artifact.clone();
    table_only.schema_version = 2;
    for stage in &mut table_only.stages {
        if let Some(program) = &mut stage.ir.lifted {
            for control in &mut program.controls {
                control.guard = None;
            }
        }
    }
    assert!(
        !table_only.admits_schema(),
        "shared rank metadata alone also requires schema 3"
    );
    // A v2 producer's four-field projection remains guard-free and byte stable
    // on decode/re-encode; current serializers do not insert new empty fields.
    let mut old = checked(&super::super::support::comparison_branch(0x10, 0));
    old.schema_version = 2;
    for stage in &mut old.stages {
        if let Some(program) = &mut stage.ir.lifted {
            for control in &mut program.controls {
                control.guard = None;
            }
            program.failures.clear();
        }
    }
    let encoded = serde_json::to_string(&old).unwrap();
    assert!(!encoded.contains("\"guard\"") && !encoded.contains("\"failures\""));
    let restored: Artifact = serde_json::from_str(&encoded).unwrap();
    assert!(restored.admits_schema());
    assert_eq!(serde_json::to_string(&restored).unwrap(), encoded);
    let old_view = render_text(&restored);
    assert!(
        old_view.contains("if (") && !old_view.contains("require("),
        "legacy v2 must retain its declared guard-free view: {old_view}"
    );
    let v1 = include_str!("../../../../evm-decompile-model/tests/fixtures/partial-artifact.json")
        .trim_end();
    let raw: Artifact = serde_json::from_str(v1).unwrap();
    assert!(raw.admits_schema());
    assert_eq!(raw.schema_version, 1);
    assert!(raw.stages.iter().all(|stage| stage.ir.lifted.is_none()));
    assert_eq!(serde_json::to_string(&raw).unwrap(), v1);
}
