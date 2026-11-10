use super::{BodyRows, E, EntityId, MAX_SHARED_REGION_ENTITIES};

#[test]
fn typed_annotations_remain_distinct_from_fault_diagnostics() {
    let mut output = BodyRows::default();
    output
        .emit(EntityId(1), "// block b1".into(), 1, true, &mut |_| true)
        .unwrap();
    output
        .emit(
            EntityId(2),
            "// fault: stack underflow".into(),
            1,
            false,
            &mut |_| true,
        )
        .unwrap();
    assert!(output.rows[0].is_structure_annotation);
    assert!(!output.rows[1].is_structure_annotation);
}

#[test]
fn removing_context_rows_does_not_refund_the_cumulative_row_limit() {
    let mut output = BodyRows {
        emitted_rows: MAX_SHARED_REGION_ENTITIES - 1,
        ..BodyRows::default()
    };
    output
        .emit(EntityId(1), "// block b1".into(), 1, true, &mut |_| true)
        .unwrap();
    output.rows.clear();
    assert_eq!(
        output.emit(EntityId(2), "evm.stop();".into(), 1, false, &mut |_| true),
        Err(E::Limit)
    );
    assert!(output.rows.is_empty());
}

#[test]
fn removing_context_rows_does_not_refund_cumulative_text_or_paid_work() {
    let mut output = BodyRows {
        text_bytes: 4_194_303,
        ..BodyRows::default()
    };
    let mut work = 0;
    let mut pay = |amount| {
        work += amount;
        true
    };
    output
        .emit(EntityId(1), "/".into(), 1, true, &mut pay)
        .unwrap();
    output.rows.clear();
    assert_eq!(
        output.emit(EntityId(2), "/".into(), 1, true, &mut pay),
        Err(E::Limit)
    );
    assert!(output.rows.is_empty());
    assert_eq!(work, 4);
}
