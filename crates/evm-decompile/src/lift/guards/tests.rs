//! Exercise the local verifier directly, without normal-form reconstruction.
use super::{discover, verify};
use crate::{
    Config, EntityId, Input, LiftedExpressionKind, LiftedProgram, Stage, StageKind, Terminator,
    U256, decompile,
};

fn projected(code: &[u8]) -> Stage {
    let artifact = decompile(
        &Input::new(code.to_vec(), crate::Fork::Osaka),
        &Config::default(),
    );
    let stage = artifact.current().expect("retained stage");
    assert_eq!(stage.kind, StageKind::Lifted, "{:?}", artifact.diagnostics);
    stage.clone()
}
fn check(stage: &Stage, program: &LiftedProgram) -> Option<bool> {
    verify(&stage.ir, program, &mut |_| true)
}
fn guarded() -> Stage {
    // True: concrete STOP; false: following empty failure suffix. CALLVALUE is
    // evaluated in the predecessor, never in the transparent failed route.
    projected(&[0x34, 0x60, 0x07, 0x57, 0x5f, 0x5f, 0xfd, 0x5b, 0x00])
}

#[test]
fn local_certificate_rejects_payload_rank_polarity_and_tail_without_projector() {
    let stage = guarded();
    let program = stage.ir.lifted.as_ref().unwrap();
    let position = program
        .controls
        .iter()
        .position(|c| c.guard.is_some())
        .unwrap();
    assert!(
        program.controls[position]
            .guard
            .as_ref()
            .unwrap()
            .continuation
            .is_some()
    );
    assert_eq!(check(&stage, program), Some(true));

    let mut corrupt = program.clone();
    corrupt.failures[0].rank = 1;
    assert_eq!(check(&stage, &corrupt), Some(false));

    let mut corrupt = program.clone();
    let guard = corrupt.controls[position].guard.as_mut().unwrap();
    guard.pass_nonzero = !guard.pass_nonzero;
    assert_eq!(check(&stage, &corrupt), Some(false));

    let mut corrupt = program.clone();
    corrupt.controls[position]
        .guard
        .as_mut()
        .unwrap()
        .continuation
        .as_mut()
        .unwrap()
        .pass_falls_through = true;
    assert_eq!(check(&stage, &corrupt), Some(false));

    let mut corrupt = program.clone();
    let failure_block = corrupt.failures.iter().find(|f| f.rank == 0).unwrap().block;
    let size = corrupt.block(failure_block).unwrap().exit_values[1];
    let expression = corrupt
        .expressions
        .iter_mut()
        .find(|e| e.id == size)
        .unwrap();
    expression.kind = LiftedExpressionKind::Word(U256::from(1));
    assert_eq!(check(&stage, &corrupt), Some(false));

    let mut corrupt = program.clone();
    corrupt.controls[position]
        .guard
        .as_mut()
        .unwrap()
        .failure_edge = EntityId(u64::MAX);
    assert_eq!(check(&stage, &corrupt), Some(false));
}

#[test]
fn proof_construction_refusal_never_publishes_partial_metadata() {
    let stage = guarded();
    let mut initial = stage.ir.lifted.clone().unwrap();
    initial.failures.clear();
    for c in &mut initial.controls {
        c.guard = None;
    }
    let mut complete = initial.clone();
    let mut total = 0_usize;
    assert_eq!(
        discover(&stage.ir, &mut complete, &mut |work| {
            total += work;
            true
        }),
        Some(())
    );
    assert_ne!(initial, complete);
    for limit in [0, total / 2, total - 1] {
        let mut candidate = initial.clone();
        let mut used = 0_usize;
        assert_eq!(
            discover(&stage.ir, &mut candidate, &mut |work| {
                used = used.saturating_add(work);
                used <= limit
            }),
            None
        );
        assert_eq!(candidate, initial, "refusal at {limit} changed metadata");
    }
}

#[test]
fn an_implicit_false_eof_stop_prevents_failure_classification() {
    // Observe condition before jumping over the REVERT to the final JUMPI.
    // Its true edge fails; its implicit false route stops at EOF.
    let stage = projected(&[
        0x34, 0x60, 0x08, 0x56, 0x5b, 0x5f, 0x5f, 0xfd, 0x5b, 0x60, 0x04, 0x57,
    ]);
    let program = stage.ir.lifted.as_ref().unwrap();
    let branch = stage
        .ir
        .blocks
        .iter()
        .find(|b| {
            matches!(
                b.terminator,
                Terminator::Branch {
                    else_edge: None,
                    ..
                }
            )
        })
        .unwrap();
    assert!(!program.failures.iter().any(|f| f.block == branch.id));
    assert!(program.controls.iter().all(|c| c.guard.is_none()));
    assert_eq!(check(&stage, program), Some(true));
}

#[test]
fn nested_guard_inherits_failure_join_without_an_ancestor_omission_plan() {
    // if (CALLVALUE) { if (calldataload(0)) { SSTORE(0,42) } }; REVERT(0,0)
    // Both failed arms reach the same outer join. The inner If's own Sequence
    // ends immediately after that If, so its closed failure suffix is inherited.
    let stage = projected(&[
        0x34, 0x60, 7, 0x57, 0x60, 24, 0x56, 0x5b, 0x5f, 0x35, 0x60, 16, 0x57, 0x60, 24, 0x56,
        0x5b, 0x60, 42, 0x5f, 0x55, 0x60, 24, 0x56, 0x5b, 0x5f, 0x5f, 0xfd,
    ]);
    let program = stage.ir.lifted.as_ref().unwrap();
    assert_eq!(
        program
            .controls
            .iter()
            .filter(|c| c.guard.is_some())
            .count(),
        2
    );
    assert_eq!(check(&stage, program), Some(true));
    let inherited = program
        .controls
        .iter()
        .position(|c| c.guard.as_ref().is_some_and(|g| g.continuation.is_none()))
        .unwrap();
    let outer = program
        .controls
        .iter()
        .filter_map(|c| c.guard.as_ref())
        .find_map(|g| g.continuation.as_ref())
        .unwrap();
    assert!(
        outer.pass_falls_through,
        "outer successful SSTORE path still reaches REVERT"
    );
    let mut corrupt = program.clone();
    let mut omission = outer.clone();
    omission.pass_falls_through = false;
    corrupt.controls[inherited]
        .guard
        .as_mut()
        .unwrap()
        .continuation = Some(omission);
    assert_eq!(
        check(&stage, &corrupt),
        Some(false),
        "an inner guard cannot delete an ancestor's join"
    );
}
