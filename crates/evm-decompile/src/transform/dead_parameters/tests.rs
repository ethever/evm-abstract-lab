//! Tuple corruption can pass local SSA/arity checks but must fail transition checks.
use crate::{
    Cancellation, Config, ExitKind, Fork, Input, Stage, StageKind, Terminator, ValidationStatus,
    ValueFact, decompile, validate::validate_transition, validate_stage,
};

fn stages(bytes: &[u8]) -> (Stage, Stage) {
    let artifact = decompile(&Input::new(bytes.to_vec(), Fork::Osaka), &Config::default());
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| { stage.validation.status == ValidationStatus::Passed })
    );
    let stage = |kind| {
        artifact
            .stages
            .iter()
            .find(|stage| stage.kind == kind)
            .unwrap()
            .clone()
    };
    (stage(StageKind::Baseline), stage(StageKind::Simplified))
}

fn rejects(before: &Stage, candidate: &Stage, message: &str) {
    assert_eq!(validate_stage(candidate).status, ValidationStatus::Passed);
    let check = validate_transition(before, candidate, &[], &Cancellation::default());
    assert!(!check.passed, "corruption must fail transition validation");
    assert!(check.message.contains(message), "{}", check.message);
}

#[test]
fn wrong_projected_argument_and_survivor_fact_are_rejected() {
    // Carry two words into a block that consumes only its top word with POP.
    let (before, after) = stages(&[0x60, 0x11, 0x60, 0x22, 0x60, 7, 0x56, 0x5b, 0x50, 0x00]);
    let old = before
        .ir
        .blocks
        .iter()
        .find(|block| block.parameters.len() == 2)
        .unwrap();
    let block = after.ir.block(old.id).unwrap();
    assert_eq!(block.parameters, old.parameters[1..]);
    let survivor = block.parameters[0];
    let edge = before
        .ir
        .edges
        .iter()
        .find(|edge| edge.to == old.id)
        .unwrap();

    let mut wrong_argument = after.clone();
    wrong_argument
        .ir
        .edges
        .iter_mut()
        .find(|e| e.id == edge.id)
        .unwrap()
        .arguments[0] = edge.arguments[0];
    rejects(&before, &wrong_argument, "incoming argument projection");

    let mut wrong_fact = after;
    wrong_fact
        .ir
        .values
        .iter_mut()
        .find(|value| value.id == survivor)
        .unwrap()
        .fact = ValueFact::Unknown;
    rejects(&before, &wrong_fact, "entry operand index/facts");
}

#[test]
fn locally_unused_open_boundary_operands_cannot_be_deleted() {
    let (before, mut after) = stages(&[0x60, 0x11, 0x60, 5, 0x56, 0x5b, 0x5f, 0x35, 0x56]);
    let boundary = after
        .ir
        .blocks
        .iter_mut()
        .find(|block| block.terminator.is_partial())
        .unwrap();
    let id = boundary.id;
    let removed = std::mem::take(&mut boundary.parameters);
    assert_eq!(removed.len(), 1);
    after.ir.values.retain(|value| !removed.contains(&value.id));
    for edge in after.ir.edges.iter_mut().filter(|edge| edge.to == id) {
        edge.arguments.clear();
    }
    rejects(&before, &after, "open boundary lost entry operands");
}

#[test]
fn established_stack_fault_cannot_turn_into_normal_termination() {
    let (before, mut after) = stages(&[0x60, 4, 0x56, 0x00, 0x5b, 0x50]);
    let block = after
        .ir
        .blocks
        .iter_mut()
        .find(|block| matches!(block.terminator, Terminator::Trap { .. }))
        .unwrap();
    block.terminator = Terminator::Exit {
        kind: ExitKind::EndOfCode,
        values: Vec::new(),
    };
    rejects(&before, &after, "established trap changed");
}
