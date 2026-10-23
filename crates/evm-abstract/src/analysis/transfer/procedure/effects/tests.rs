use super::*;
use crate::{
    analysis::machine::fixtures::{address, fixture},
    domain::{ProductAlgebra, ValueAlgebra, relational::QueryReason},
    resource::WorkBudget,
};

mod calls;

#[test]
fn complete_program_proof_vetoes_unvisited_effects_and_ignores_push_data() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let (_, entry) = fixture(&[(0x101, "00")]);
    let mut budget = WorkBudget::new(1_000_000);
    let mut cx = PolicyContext::new(values.borrowed(), &entry, &mut budget);
    // STOP makes the following opcode unreachable, but it still vetoes the
    // full-program proof. Every child CALL, including STATICCALL, can affect
    // the store/touch/log relation and is rejected conservatively.
    for op in [
        0x55, 0x5d, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xf0, 0xf1, 0xf2, 0xf4, 0xf5, 0xfa, 0xff, 0xaa,
    ] {
        let program = Program::decode(&[0x00, op]).unwrap();
        assert_eq!(
            classify(
                FrameCode::Runtime,
                FrameCode::Runtime,
                Some(&program),
                false,
                &mut cx
            )
            .unwrap(),
            StoreEffect::MayChange,
            "opcode={op:02x}"
        );
    }
    let program = Program::decode(&[0x61, 0x55, 0xf4, 0x00]).unwrap();
    assert!(matches!(
        classify(
            FrameCode::Runtime,
            FrameCode::Runtime,
            Some(&program),
            false,
            &mut cx
        )
        .unwrap(),
        StoreEffect::Unchanged(_)
    ));
}

#[test]
fn unknown_creation_and_nonruntime_frames_never_gain_the_frame_rule() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let (_, entry) = fixture(&[(0x101, "00")]);
    let program = Program::from_hex("00").unwrap();
    let mut budget = WorkBudget::new(1_000_000);
    let mut cx = PolicyContext::new(values.borrowed(), &entry, &mut budget);
    for code in [
        FrameCode::Unknown,
        FrameCode::InitCode,
        FrameCode::Empty,
        FrameCode::Precompile(address(1)),
        FrameCode::InvalidDelegation,
    ] {
        assert_eq!(
            classify(code, code, Some(&program), false, &mut cx).unwrap(),
            StoreEffect::MayChange
        );
    }
    assert_eq!(
        classify(
            FrameCode::Runtime,
            FrameCode::InitCode,
            Some(&program),
            false,
            &mut cx
        )
        .unwrap(),
        StoreEffect::MayChange
    );
    assert_eq!(
        classify(FrameCode::Runtime, FrameCode::Runtime, None, false, &mut cx).unwrap(),
        StoreEffect::MayChange
    );
    assert_eq!(
        classify(
            FrameCode::Runtime,
            FrameCode::Runtime,
            Some(&program),
            true,
            &mut cx
        )
        .unwrap(),
        StoreEffect::MayChange
    );
    let proof = classify(
        FrameCode::Runtime,
        FrameCode::Runtime,
        Some(&program),
        false,
        &mut cx,
    )
    .unwrap();
    let keeps = |kind, creation| {
        proof.keeps_store(
            FrameCode::Runtime,
            FrameCode::Runtime,
            Some(&program),
            kind,
            creation,
        )
    };
    assert!(keeps(OutcomeKind::Return, false));
    for kind in [OutcomeKind::Revert, OutcomeKind::Failure] {
        assert!(!keeps(kind, false));
    }
    assert!(!keeps(OutcomeKind::Return, true));
}

#[test]
fn cached_proof_matches_actual_program_and_fork_independently_of_region_keys() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let (_, entry) = fixture(&[(0x101, "00")]);
    let program = Program::from_hex("00").unwrap();
    let alias = program.clone();
    let effectful = Program::from_hex("60095f5500").unwrap();
    let mut budget = WorkBudget::new(1_000_000);
    let mut cx = PolicyContext::new(values.borrowed(), &entry, &mut budget);
    let proof = classify(
        FrameCode::Runtime,
        FrameCode::Runtime,
        Some(&program),
        false,
        &mut cx,
    )
    .unwrap();
    assert!(proof.keeps_store(
        FrameCode::Runtime,
        FrameCode::Runtime,
        Some(&alias),
        OutcomeKind::Return,
        false
    ));
    assert!(!proof.keeps_store(
        FrameCode::Runtime,
        FrameCode::Runtime,
        Some(&effectful),
        OutcomeKind::Return,
        false
    ));
    assert!(!proof.keeps_store(
        FrameCode::Unknown,
        FrameCode::Runtime,
        Some(&alias),
        OutcomeKind::Return,
        false
    ));
    assert!(!proof.keeps_store(
        FrameCode::Runtime,
        FrameCode::InitCode,
        Some(&alias),
        OutcomeKind::Return,
        false
    ));
    // Empty byte slices may share a dangling identity; fork remains an
    // independent semantic condition even for that zero-length corner.
    let empty = Program::decode_with_fork(&[], crate::Fork::Osaka).unwrap();
    let foreign_fork = Program::decode_with_fork(&[], crate::Fork::Cancun).unwrap();
    let proof = classify(
        FrameCode::Runtime,
        FrameCode::Runtime,
        Some(&empty),
        false,
        &mut cx,
    )
    .unwrap();
    assert!(!proof.keeps_store(
        FrameCode::Runtime,
        FrameCode::Runtime,
        Some(&foreign_fork),
        OutcomeKind::Return,
        false
    ));
}

#[test]
fn scan_refusal_or_captured_cancellation_cannot_publish_an_unchanged_proof() {
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let (_, entry) = fixture(&[(0x101, "00")]);
    let program = Program::from_hex("600060005260206000f3").unwrap();
    let mut budget = WorkBudget::new(1);
    let mut cx = PolicyContext::new(values.borrowed(), &entry, &mut budget);
    assert!(matches!(
        classify(
            FrameCode::Runtime,
            FrameCode::Runtime,
            Some(&program),
            false,
            &mut cx
        ),
        Err(MachineError::Work)
    ));
    assert_eq!(cx.budget.used(), 1);
    let token = analysis_control::Cancellation::new();
    let mut budget = analysis_control::with_cancellation(&token, || WorkBudget::new(1_000_000));
    token.cancel();
    let mut cx = PolicyContext::new(values.borrowed(), &entry, &mut budget);
    assert!(matches!(
        classify(
            FrameCode::Runtime,
            FrameCode::Runtime,
            Some(&program),
            false,
            &mut cx
        ),
        Err(MachineError::Relations(QueryReason::Cancelled))
    ));
    assert_eq!(cx.budget.used(), 0);
}
