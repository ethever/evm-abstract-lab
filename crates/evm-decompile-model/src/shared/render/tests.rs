use super::{BodyRows, E, EntityId, MAX_SHARED_REGION_ENTITIES};

#[test]
fn nested_empty_else_keeps_the_inner_if_and_the_outer_else() {
    let mut output = BodyRows::default();
    let mut pay = |_| true;
    output
        .emit(EntityId(1), "if (outer) {".into(), 0, false, &mut pay)
        .unwrap();
    let outer = output.begin_else(EntityId(1), 0, &mut pay).unwrap();
    output
        .emit(EntityId(2), "if (inner) {".into(), 1, false, &mut pay)
        .unwrap();
    let inner = output.begin_else(EntityId(2), 1, &mut pay).unwrap();
    output
        .emit(EntityId(3), "// block b3".into(), 2, true, &mut pay)
        .unwrap();
    output.end_else(inner);
    output
        .emit(EntityId(2), "}".into(), 1, false, &mut pay)
        .unwrap();
    output.end_else(outer);
    output
        .emit(EntityId(1), "}".into(), 0, false, &mut pay)
        .unwrap();
    let rows: Vec<_> = output
        .rows
        .into_iter()
        .map(|row| (row.entity.0, row.depth, row.text))
        .collect();
    assert_eq!(
        rows,
        [
            (1, 0, "if (outer) {".into()),
            (1, 0, "} else {".into()),
            (2, 1, "if (inner) {".into()),
            (2, 1, "}".into()),
            (1, 0, "}".into()),
        ]
    );
}

#[test]
fn an_empty_shared_if_keeps_its_condition_and_closing_brace() {
    let mut output = BodyRows::default();
    let mut pay = |_| true;
    output
        .emit(EntityId(1), "if (condition) {".into(), 0, false, &mut pay)
        .unwrap();
    let otherwise = output.begin_else(EntityId(1), 0, &mut pay).unwrap();
    output.end_else(otherwise);
    output
        .emit(EntityId(1), "}".into(), 0, false, &mut pay)
        .unwrap();
    assert_eq!(output.rows.len(), 2);
    assert_eq!(output.rows[0].text, "if (condition) {");
    assert_eq!(output.rows[1].text, "}");
}

#[test]
fn typed_annotations_are_context_and_fault_comments_are_content() {
    let mut output = BodyRows::default();
    let checkpoint = output.summary;
    output
        .emit(EntityId(1), "// block b1".into(), 1, true, &mut |_| true)
        .unwrap();
    assert!(!output.summary.since(checkpoint).has_content());
    output
        .emit(
            EntityId(2),
            "// fault: stack underflow".into(),
            1,
            false,
            &mut |_| true,
        )
        .unwrap();
    assert!(output.summary.since(checkpoint).has_content());
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
        output.emit(EntityId(2), "stop();".into(), 1, false, &mut |_| true),
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
