use crate::{
    Cancellation, Config, EntityId, Fork, Input, SharedProcedureError, StageKind, decompile,
};

fn witness() -> crate::Artifact {
    // Two arms with independent terminal exits materialize the useful common
    // tail twice. This fixture tests syntax occurrences, not source-PC equality.
    let mut code = Vec::new();
    let mut patches = Vec::new();
    fn branch(code: &mut Vec<u8>, patches: &mut Vec<(usize, bool)>, offset: u8, left: bool) {
        code.extend([0x60, offset, 0x35, 0x61, 0, 0, 0x57]);
        patches.push((code.len() - 3, left));
    }
    branch(&mut code, &mut patches, 0, true);
    branch(&mut code, &mut patches, 32, false);
    code.push(0x00);
    let left = code.len();
    code.push(0x5b);
    branch(&mut code, &mut patches, 64, false);
    code.push(0x00);
    let tail = code.len();
    code.extend([
        0x5b, 0x60, 96, 0x35, 0x80, 0x60, 64, 0x52, 0x60, 5, 0x55, 0x60, 32, 0x60, 64, 0xa0, 0x60,
        9, 0x60, 8, 0x55, 0x00,
    ]);
    for (at, is_left) in patches {
        let pc = u16::try_from(if is_left { left } else { tail }).unwrap();
        code[at..at + 2].copy_from_slice(&pc.to_be_bytes());
    }
    let a = decompile(&Input::new(code, Fork::Osaka), &Config::default());
    assert_eq!(
        a.current().unwrap().kind,
        StageKind::Lifted,
        "{:?}",
        a.diagnostics
    );
    assert!(
        !a.current()
            .unwrap()
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .procedures
            .is_empty()
    );
    a
}
#[test]
fn direct_supplied_map_rejection_quarantines_omission_and_keeps_source_anchors() {
    let a = witness();
    let stage = a.current().unwrap();
    let p = stage.ir.lifted.as_ref().unwrap();
    assert!(p.validate_shared_sources(&stage.ir, |_| true).is_ok());
    for id in p
        .procedures
        .iter()
        .map(|p| p.id)
        .chain(p.invocations.iter().map(|i| i.id))
    {
        assert!(
            !a.origin(crate::EntityRef {
                stage: stage.id,
                entity: id
            })
            .unwrap()
            .source
            .is_empty()
        );
    }
    let mut corrupt = p.clone();
    let call = corrupt.invocations.last_mut().unwrap();
    call.owner = Some(EntityId(u64::MAX));
    assert!(matches!(
        corrupt.validate_shared_sources(&stage.ir, |_| true),
        Err(SharedProcedureError::Site(_))
    ));
    let render = corrupt.render_index(&stage.ir);
    assert!(render.validate_shared_sources().is_err());
    assert_eq!(render.procedures().count(), 0);
    for i in &p.invocations {
        assert!(render.shared_invocation(i.source).is_none());
    }
    let structured = a
        .stages
        .iter()
        .find(|s| s.kind == StageKind::Structured)
        .unwrap();
    assert_eq!(structured.ir.blocks, stage.ir.blocks);
    assert_eq!(structured.ir.edges, stage.ir.edges);
    assert_eq!(structured.ir.structured, stage.ir.structured);
}
#[test]
fn sparse_identity_verification_is_paid_before_source_index_and_map_work() {
    let a = witness();
    let ir = &a.current().unwrap().ir;
    let p = ir.lifted.as_ref().unwrap();
    let mut reservations = Vec::new();
    let result = p.validate_shared_sources(ir, |n| {
        reservations.push(n);
        false
    });
    assert_eq!(result, Err(SharedProcedureError::Limit));
    assert_eq!(reservations.len(), 1);
    let render = p.render_index_with_budget(ir, |_| false);
    assert!(render.validate_shared_sources().is_err());
    let export = crate::export_text_with_observer(&a, &Cancellation::default(), 1, |_| {});
    assert_eq!(export.status, crate::TextExportStatus::Limited);
}
#[test]
fn duplicate_invocation_identity_fails_model_and_direct_render_admission() {
    let a = witness();
    let ir = &a.current().unwrap().ir;
    let mut p = ir.lifted.clone().unwrap();
    assert!(p.invocations.len() >= 2);
    p.invocations[1].id = p.invocations[0].id;
    assert!(p.validate().is_err());
    assert!(p.validate_shared_sources(ir, |_| true).is_err());
    assert_eq!(p.render_index(ir).procedures().count(), 0);
}

#[test]
fn source_primitive_duplicates_and_effectful_replacements_cannot_hide_behind_shared_maps() {
    let a = witness();
    let source = &a.current().unwrap().ir;
    let store = source
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|o| {
            o.opcode == 0x55
                && source
                    .lifted
                    .as_ref()
                    .unwrap()
                    .invocations
                    .iter()
                    .any(|i| i.actual(o.id) == Some(o.id))
        })
        .unwrap()
        .id;
    let mut duplicate = source.clone();
    let block = duplicate
        .blocks
        .iter_mut()
        .find(|b| b.operations.iter().any(|o| o.id == store))
        .unwrap();
    let position = block.operations.iter().position(|o| o.id == store).unwrap();
    block
        .operations
        .insert(position, block.operations[position].clone());
    assert!(
        duplicate
            .lifted
            .as_ref()
            .unwrap()
            .validate_shared_sources(&duplicate, |_| true)
            .is_err()
    );
    let mut specialized = source.clone();
    let operation = specialized
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|o| o.id == store)
        .unwrap();
    operation.replacement = Some(crate::U256::ZERO);
    assert!(
        specialized
            .lifted
            .as_ref()
            .unwrap()
            .validate_shared_sources(&specialized, |_| true)
            .is_err()
    );
}

#[test]
fn canonical_loop_rows_preserve_simultaneous_words_and_state_transfers() {
    let hex = "61000a600035610021565b610015602035610021565b60205260005260406000f35b60005b8115610056578101806040528160025560206040a0602061014060206040611234620186a0fa50906001900390610024565b90509056";
    let code = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    let a = decompile(&Input::new(code, Fork::Osaka), &Config::default());
    let ir = &a.current().unwrap().ir;
    let p = ir.lifted.as_ref().unwrap();
    let render = p.render_index(ir);
    let mut transfers = 0;
    for d in render.procedures() {
        let shown = render.render_procedure_body(d, true, |_| true).unwrap();
        let hidden = render.render_procedure_body(d, false, |_| true).unwrap();
        for row in shown.iter().filter(|row| ir.edge(row.entity).is_some()) {
            let edge = ir.edge(row.entity).unwrap();
            let target = ir.block(edge.to).unwrap();
            for text in [
                format!(
                    "m{} = m{}; // memory token on e{}",
                    target.memory_parameter.0, edge.memory.0, edge.id.0
                ),
                format!(
                    "q{} = q{}; // effect token on e{}",
                    target.effect_parameter.0, edge.effect.0, edge.id.0
                ),
            ] {
                assert!(shown.iter().any(|r| r.entity == edge.id && r.text == text));
                assert!(!hidden.iter().any(|r| r.entity == edge.id && r.text == text));
            }
            transfers += 1;
        }
    }
    assert!(
        transfers > 0,
        "loop sharing must expose real source transfer rows"
    );
}

#[test]
fn closed_fragment_exports_values_used_by_a_following_partial_target() {
    let a = decompile(
        &Input::new(
            vec![0x5f, 0x35, 0x60, 0x06, 0x56, 0x00, 0x5b, 0x56],
            Fork::Osaka,
        ),
        &Config::default(),
    );
    let ir = &a.current().unwrap().ir;
    let raw = ir
        .blocks
        .iter()
        .find(|b| b.source.is_some_and(|s| s.pc == 0))
        .unwrap();
    let value = raw
        .operations
        .iter()
        .find(|o| o.opcode == 0x35)
        .unwrap()
        .outputs[0];
    assert!(ir.lifted.is_some());
    let mut todo = vec![&ir.structured.as_ref().unwrap().root];
    let mut node = None;
    while let Some(n) = todo.pop() {
        if matches!(n.kind,crate::StructuredKind::Block{block}if block==raw.id) {
            node = Some(n.id);
            break;
        }
        super::super::children(n, &mut todo);
    }
    let index = crate::SharedSourceIndex::new(ir, &mut |_| true).unwrap();
    let region = index.region(node.unwrap(), &mut |_| true).unwrap();
    assert!(
        region.interface.outputs.contains(&value),
        "explicit unresolved target still consumes the produced high Value"
    );
}

#[test]
fn wide_parallel_rows_refuse_before_expanding_every_rhs() {
    use crate::{
        LiftedBlock, LiftedExpression, LiftedExpressionKind as X, LiftedProgram, LiftedTransfer,
        LiftedType,
    };
    let mut expressions = vec![LiftedExpression {
        id: EntityId(1),
        block: EntityId(10),
        ty: LiftedType::Word256,
        kind: X::Word(crate::U256::from(1)),
    }];
    for n in 2_u64..4098 {
        expressions.push(LiftedExpression {
            id: EntityId(n),
            block: EntityId(10),
            ty: LiftedType::Word256,
            kind: X::WordOp {
                opcode: 0x01,
                arguments: vec![EntityId(n - 1), EntityId(1)],
            },
        });
    }
    let target = LiftedBlock {
        id: EntityId(5000),
        block: EntityId(20),
        parameters: (0_u64..1000).map(|n| EntityId(10_000 + n)).collect(),
        statements: Vec::new(),
        condition: None,
        target: None,
        exit_values: Vec::new(),
    };
    let transfer = LiftedTransfer {
        id: EntityId(5001),
        edge: EntityId(30),
        arguments: vec![EntityId(4097); 1000],
    };
    let program = LiftedProgram {
        expressions,
        blocks: vec![target],
        transfers: vec![transfer],
        ..LiftedProgram::default()
    };
    let ir = crate::Ir {
        edges: vec![crate::Edge {
            id: EntityId(30),
            from: EntityId(10),
            to: EntityId(20),
            kind: crate::EdgeKind::Jump,
            arguments: Vec::new(),
            memory: EntityId(31),
            effect: EntityId(32),
        }],
        ..crate::Ir::default()
    };
    let renderer = program.render_base_index_with_budget(&ir, |_| true);
    let mut work = 0_usize;
    let result = renderer.render_transfer(&program.transfers[0], |n| {
        work += n;
        true
    });
    assert_eq!(result, Err(crate::LiftedRenderError::Limit));
    assert!(
        work < 1_000_000,
        "a refused row must not expand all 1000 large RHS expressions: {work}"
    );
}

#[test]
fn unshared_base_validation_never_constructs_a_shared_flow_index() {
    let a = witness();
    let ir = &a.current().unwrap().ir;
    let mut base = ir.lifted.clone().unwrap();
    base.procedures.clear();
    base.invocations.clear();
    let mut calls = 0;
    assert_eq!(
        base.validate_shared_sources(ir, |_| {
            calls += 1;
            false
        }),
        Ok(())
    );
    assert_eq!(calls, 0);
}

#[test]
fn discovery_distinguishes_invalid_base_witness_from_optional_capacity() {
    let a = witness();
    let ir = &a.current().unwrap().ir;
    let mut base = ir.lifted.clone().unwrap();
    base.procedures.clear();
    base.invocations.clear();
    base.blocks.clear();
    let mut next = u64::MAX / 2;
    let result = super::discover(ir, &base, &mut next, usize::MAX, &mut |_| true);
    assert!(
        matches!(result, Err(super::DiscoveryError::Invalid(_))),
        "{result:?}"
    );
}

fn structured_owner(
    input: &Input,
    cancel: &Cancellation,
) -> crate::artifact::Builder<impl FnMut(crate::Progress) + use<>> {
    fn ignore(_: crate::Progress) {}
    let mut owner = crate::artifact::Builder::new(input, &Config::default(), cancel, ignore);
    assert!(crate::recover::run(&mut owner));
    crate::transform::simplify(&mut owner);
    crate::structure::run(&mut owner);
    assert_eq!(
        owner.current().kind,
        StageKind::Structured,
        "{:?}",
        owner.artifact.diagnostics
    );
    owner
}

#[test]
fn optional_sharing_work_refusal_publishes_the_complete_validated_base_only() {
    let expected = witness();
    let mut owner = structured_owner(&expected.input, &Cancellation::default());
    let prior_stages = owner.artifact.stages.len();
    super::super::run_with_sharing_limits(&mut owner, 0, usize::MAX);
    let result = owner.finish();
    assert_eq!(
        result.completion,
        crate::Completion::Complete,
        "{:?}",
        result.diagnostics
    );
    assert_eq!(result.current().unwrap().kind, StageKind::Lifted);
    assert_eq!(result.stages.len(), prior_stages + 1);
    assert!(
        result
            .stages
            .iter()
            .all(|stage| stage.validation.status == crate::ValidationStatus::Passed)
    );
    let actual = result.current().unwrap().ir.lifted.as_ref().unwrap();
    assert!(actual.procedures.is_empty() && actual.invocations.is_empty());
    let mut base = expected.current().unwrap().ir.lifted.clone().unwrap();
    base.procedures.clear();
    base.invocations.clear();
    assert_eq!(*actual, base);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == crate::DiagnosticCode::OptimizationRefused
                && d.severity == crate::Severity::Info
                && d.message.contains("validated base Lifted retained"))
    );
}

#[test]
fn cancellation_and_total_work_refusal_still_stop_before_lift_publication() {
    let expected = witness();
    let cancel = Cancellation::new();
    let mut cancelled = structured_owner(&expected.input, &cancel);
    cancel.cancel();
    super::super::run_with_sharing_limits(&mut cancelled, 0, usize::MAX);
    assert_eq!(cancelled.artifact.completion, crate::Completion::Cancelled);
    assert_eq!(cancelled.current().kind, StageKind::Structured);
    let mut limited = structured_owner(&expected.input, &Cancellation::default());
    limited.budget.limit_remaining(0).unwrap();
    super::super::run_with_sharing_limits(&mut limited, 0, usize::MAX);
    assert_eq!(limited.artifact.completion, crate::Completion::Limited);
    assert_eq!(limited.current().kind, StageKind::Structured);
    assert!(
        limited
            .artifact
            .diagnostics
            .iter()
            .all(|d| d.code != crate::DiagnosticCode::OptimizationRefused)
    );
}

#[test]
fn optional_flow_capacity_refusal_keeps_the_complete_certified_base() {
    let expected = witness();
    let mut owner = structured_owner(&expected.input, &Cancellation::default());
    super::super::run_with_sharing_limits(&mut owner, usize::MAX, 0);
    let result = owner.finish();
    assert_eq!(
        result.completion,
        crate::Completion::Complete,
        "{:?}",
        result.diagnostics
    );
    let stage = result.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted);
    assert_eq!(stage.validation.status, crate::ValidationStatus::Passed);
    let actual = stage.ir.lifted.as_ref().unwrap();
    assert!(actual.procedures.is_empty() && actual.invocations.is_empty());
    let mut base = expected.current().unwrap().ir.lifted.clone().unwrap();
    base.procedures.clear();
    base.invocations.clear();
    assert_eq!(*actual, base);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == crate::DiagnosticCode::OptimizationRefused
                && d.message.contains("FlowEventSlots")
                && d.message.contains("limit 0"))
    );
}
