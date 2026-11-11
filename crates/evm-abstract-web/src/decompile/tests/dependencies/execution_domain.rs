//! Declared transaction premises survive submission, publication and copy.
use super::*;
use crate::{
    JobOperation,
    decompile::{domain, task},
};
use evm_abstract_protocol::decompile_model::ExecutionDomain;
use evm_abstract_protocol::{DecompileJobResult, DecompileJobResultReply};

fn deliver_scope(
    workspace: &mut Workspace,
    artifact: DecompileArtifact,
    requested: ExecutionDomain,
    job: u64,
) {
    let request = DecompileRequest {
        bytecode: artifact
            .input
            .runtime_bytecode
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        execution_domain: requested,
        ..Default::default()
    };
    let task::Command::Submit { generation, .. } = workspace.task.start(request) else {
        unreachable!()
    };
    workspace.receive(task::Message::Status {
        generation,
        operation: JobOperation::Submit,
        result: Box::new(Ok(crate::decompile::tests::fixture::snapshot(
            job,
            JobState::Completed,
            Some(crate::decompile::tests::fixture::version(1)),
        ))),
    });
    assert!(matches!(
        workspace.task.next(0.0),
        Some(task::Command::Result { .. })
    ));
    workspace.receive(task::Message::Result {
        generation,
        id: evm_abstract_protocol::JobId(job),
        result: Box::new(Ok(DecompileJobResultReply {
            result: Ok(DecompileJobResult {
                id: evm_abstract_protocol::JobId(job),
                publication: crate::decompile::tests::fixture::version(1),
                artifact,
            }),
        })),
    });
}

#[test]
fn declared_execution_scope_survives_copy_and_rejected_publications() {
    let generic = artifact("00");
    let selected = artifact_with_request(DecompileRequest {
        bytecode: "00".into(),
        execution_domain: ExecutionDomain::EthereumTransactions,
        ..Default::default()
    });
    let (ctx, mut workspace) = setup(generic.clone());
    let original_copy = click(&ctx, &mut workspace, "Copy structured text").copied;
    deliver_scope(
        &mut workspace,
        selected.clone(),
        ExecutionDomain::Generic,
        11,
    );
    assert_eq!(workspace.artifact.as_ref(), Some(&generic));
    assert!(
        workspace
            .accessible_status()
            .contains("Execution scope mismatch")
    );
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        original_copy
    );
    deliver_scope(
        &mut workspace,
        selected.clone(),
        ExecutionDomain::EthereumTransactions,
        12,
    );
    assert_eq!(workspace.artifact.as_ref(), Some(&selected));
    assert!(
        workspace
            .accessible_status()
            .contains("Execution scope: Ethereum valid transactions")
    );
    let visible = frame(&ctx, &mut workspace, vec![]);
    assert!(
        visible
            .controls
            .iter()
            .any(|row| row["role"] == "decompile-domain"
                && row["label"] == "Execution scope: Ethereum valid transactions")
    );
    let comment = domain::source_comment(ExecutionDomain::EthereumTransactions).unwrap();
    for dependencies in [false, true] {
        for annotations in [false, true] {
            let lines = guards::rows_with(
                selected.current().unwrap(),
                structured::HighDisplayOptions {
                    show_state_dependencies: dependencies,
                    show_structure_annotations: annotations,
                },
            );
            assert_eq!(lines[0].text, comment);
            assert_eq!(copied_text(&lines).matches(comment).count(), 1);
        }
    }
    let selected_copy = click(&ctx, &mut workspace, "Copy structured text").copied;
    assert!(selected_copy[0].starts_with(comment));
    for fork_mismatch in [false, true] {
        let mut forged = selected.clone();
        let stage = forged.stages.last_mut().unwrap();
        if fork_mismatch {
            stage.ir.execution_fork = Some(evm_abstract_protocol::decompile_model::Fork::Cancun);
        } else {
            stage.ir.execution_domain = ExecutionDomain::Generic;
            stage.ir.execution_fork = None;
        }
        assert!(model::check_artifact(&forged).is_err());
        deliver_scope(
            &mut workspace,
            forged,
            ExecutionDomain::EthereumTransactions,
            13 + u64::from(fork_mismatch),
        );
        assert_eq!(workspace.artifact.as_ref(), Some(&selected));
        assert_eq!(
            click(&ctx, &mut workspace, "Copy structured text").copied,
            selected_copy
        );
    }
}

#[test]
fn pair_constructor_keeps_its_scope_sources_and_complete_copied_text() {
    let code = include_str!("../../../../../evm-decompile/tests/fixtures/error-payloads/pair-returndata-allocation-guard.hex").trim();
    let generic = artifact(code);
    assert!(
        !generic
            .current()
            .unwrap()
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .payloads
            .iter()
            .any(|payload| payload.message == "UniswapV2: TRANSFER_FAILED")
    );
    let selected = artifact_with_request(DecompileRequest {
        bytecode: code.into(),
        execution_domain: ExecutionDomain::EthereumTransactions,
        ..Default::default()
    });
    let stage = selected.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let payload = program
        .payloads
        .iter()
        .find(|payload| payload.message == "UniswapV2: TRANSFER_FAILED")
        .unwrap();
    let proof = program
        .reverts
        .iter()
        .find(|proof| proof.payload == payload.id)
        .unwrap();
    assert_eq!(
        proof.execution_domain,
        ExecutionDomain::EthereumTransactions
    );
    assert_eq!(proof.execution_fork, Some(selected.input.fork));
    assert_eq!(proof.writes.len(), 4);
    let renderer = program.render_index(&stage.ir);
    renderer.validate_guard_sources().unwrap();
    let construction = renderer.construction(proof.block).unwrap();
    let (ctx, mut workspace) = setup(generic);
    deliver_scope(
        &mut workspace,
        selected.clone(),
        ExecutionDomain::EthereumTransactions,
        11,
    );
    let mut display = structured::HighDisplayOptions::default();
    for (dependencies, annotations) in [
        (false, false),
        (true, false),
        (true, true),
        (false, true),
        (false, false),
    ] {
        if dependencies != display.show_state_dependencies {
            click(&ctx, &mut workspace, "Show state dependencies");
        }
        if annotations != display.show_structure_annotations {
            click(&ctx, &mut workspace, "Show structure annotations");
        }
        display = structured::HighDisplayOptions {
            show_state_dependencies: dependencies,
            show_structure_annotations: annotations,
        };
        let lines = guards::rows_with(stage, display);
        let text = copied_text(&lines);
        assert!(
            text.starts_with(
                domain::source_comment(ExecutionDomain::EthereumTransactions).unwrap()
            )
        );
        assert!(text.contains("evm.mstore(offset =") && text.contains("evm.revert(offset ="));
        assert_painted(&frame(&ctx, &mut workspace, vec![]), &lines);
        assert_eq!(
            click(&ctx, &mut workspace, "Copy structured text").copied,
            [text]
        );
    }
    for entity in [payload.id, proof.id, construction.id] {
        let at = EntityRef {
            stage: stage.id,
            entity,
        };
        let source = selected.origin(at).unwrap().source[0].pc;
        assert!(
            workspace
                .state
                .source_descendants(&selected, source)
                .contains(&at)
        );
        assert!(workspace.state.navigate(&selected, at));
        let details = workspace.state.details(&selected, at);
        assert!(
            details.lines[0].starts_with("Certified"),
            "{:?}",
            details.lines
        );
    }
    let statement = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| statement.operation == proof.operation)
        .unwrap();
    let at = EntityRef {
        stage: stage.id,
        entity: statement.id,
    };
    assert!(workspace.state.navigate(&selected, at));
    let visible = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "Simplified structured code");
    assert!(visible.controls.iter().any(|row| {
        row["role"] == "decompile-code"
            && row["selected"] == true
            && row["label"]
                .as_str()
                .unwrap()
                .contains("evm.revert(offset =")
    }));
    let retained = click(&ctx, &mut workspace, "Copy structured text").copied;
    let mut erased = selected.clone();
    erased.config.execution_domain = ExecutionDomain::Generic;
    for stage in &mut erased.stages {
        stage.ir.execution_domain = ExecutionDomain::Generic;
        stage.ir.execution_fork = None;
    }
    assert!(
        !erased.admits_schema(),
        "the certificate still declares its transaction premise"
    );
    assert!(model::check_artifact(&erased).is_err());
    deliver_scope(&mut workspace, erased, ExecutionDomain::Generic, 12);
    assert!(matches!(
        workspace.task.error.as_ref(),
        Some(task::Error::Artifact(_))
    ));
    assert_eq!(workspace.artifact.as_ref(), Some(&selected));
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        retained
    );
    let mut relabelled = selected.clone();
    relabelled.input.fork = evm_abstract_protocol::decompile_model::Fork::Cancun;
    relabelled.readability.clear();
    for stage in &mut relabelled.stages {
        stage.ir.execution_fork = Some(relabelled.input.fork);
        if let Some(program) = &mut stage.ir.lifted {
            for proof in &mut program.reverts {
                proof.execution_fork = Some(relabelled.input.fork);
            }
            for data in &mut program.revert_data {
                data.execution_fork = Some(relabelled.input.fork);
            }
        }
    }
    assert!(relabelled.admits_schema());
    model::check_artifact(&relabelled).unwrap();
    deliver_scope(
        &mut workspace,
        relabelled,
        ExecutionDomain::EthereumTransactions,
        13,
    );
    assert!(matches!(
        workspace.task.error.as_ref(),
        Some(task::Error::ForkMismatch { .. })
    ));
    assert_eq!(workspace.artifact.as_ref(), Some(&selected));
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        retained
    );
}
