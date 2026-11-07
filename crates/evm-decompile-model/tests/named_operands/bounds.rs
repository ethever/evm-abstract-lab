use super::super::{expression, primitive, words};
use evm_decompile_model::{
    EntityId, LiftedExpressionKind as Kind, LiftedProgram, LiftedRenderError,
    MAX_LIFTED_EXPRESSION_TEXT,
};

#[test]
fn labels_are_paid_work_and_refusal_cannot_return_a_partial_invocation() {
    let (program, ir, statement) = primitive(0xf1, "CALL", 7);
    let (positional_program, positional_ir, positional_statement) = primitive(0x0c, "CALL", 7);
    let renderer = program.render_index(&ir);
    let mut named_work = 0;
    let named = renderer
        .render_statement(&statement, |units| {
            named_work += units;
            true
        })
        .unwrap();
    let mut positional_work = 0;
    let positional = positional_program
        .render_index(&positional_ir)
        .render_statement(&positional_statement, |units| {
            positional_work += units;
            true
        })
        .unwrap();
    let label_bytes = "gas = ".len()
        + "target = ".len()
        + "value = ".len()
        + "input_offset = ".len()
        + "input_size = ".len()
        + "output_offset = ".len()
        + "output_size = ".len();
    assert_eq!(named.len(), positional.len() + label_bytes);
    assert!(
        named_work >= positional_work + label_bytes,
        "label text escaped the work ledger: named={named_work}, positional={positional_work}"
    );

    for allowance in [0, 1, named_work / 2, named_work - 1] {
        let mut paid = 0;
        let result = renderer.render_statement(&statement, |units| {
            if units > allowance - paid {
                return false;
            }
            paid += units;
            true
        });
        assert_eq!(
            result,
            Err(LiftedRenderError::Limit),
            "allowance={allowance}"
        );
        assert!(paid <= allowance);
    }
    assert_eq!(
        renderer.render_statement(&statement, |_| true).unwrap(),
        named
    );
}

#[test]
fn even_empty_primitives_observe_refusal_and_a_cancelled_reservation() {
    for (opcode, name, count) in [(0x00, "STOP", 0), (0xf1, "CALL", 7)] {
        let (program, ir, statement) = primitive(opcode, name, count);
        let renderer = program.render_index(&ir);
        for dependencies in [false, true] {
            assert_eq!(
                renderer.render_statement_with_dependencies(&statement, dependencies, |_| false),
                Err(LiftedRenderError::Limit),
                "{name}"
            );
        }
    }
    let (program, ir, statement) = primitive(0xf1, "CALL", 7);
    let original = (program.clone(), ir.clone(), statement.clone());
    let renderer = program.render_index(&ir);
    let mut total_work = 0;
    renderer
        .render_statement(&statement, |units| {
            total_work += units;
            true
        })
        .unwrap();
    let threshold = total_work / 2;
    let mut paid = 0;
    let mut cancelled = false;
    let result = renderer.render_statement(&statement, |units| {
        if cancelled || units > threshold - paid {
            cancelled = true;
            return false;
        }
        paid += units;
        true
    });
    assert!(cancelled);
    assert!(paid <= threshold);
    assert_eq!(result, Err(LiftedRenderError::Limit));
    assert_eq!((program, ir, statement), original);
}

fn shared_exponents(depth: u64) -> (LiftedProgram, EntityId) {
    let mut program = words(1);
    for id in 2..=depth + 1 {
        program.expressions.push(expression(
            EntityId(id),
            Kind::WordOp {
                opcode: 0x0a,
                arguments: vec![EntityId(id - 1), EntityId(id - 1)],
            },
        ));
    }
    program.validate().unwrap();
    (program, EntityId(depth + 1))
}

#[test]
fn expression_size_limit_counts_nested_labels_and_separators() {
    let (program, root) = shared_exponents(12);
    // The former positional expansion is 45,049 bytes; naming base/exponent
    // expands this same small shared DAG beyond the 65,536-byte ceiling.
    let positional_bytes = 11 * (1 << 12) - 7;
    assert!(positional_bytes < MAX_LIFTED_EXPRESSION_TEXT);
    let ir = Default::default();
    assert_eq!(
        program.render_index(&ir).render_expression(root, |_| true),
        Err(LiftedRenderError::Limit)
    );
    assert_eq!(
        program.render_expression(root),
        format!("expression_limit(e{})", root.0)
    );
}

#[test]
fn statement_size_includes_the_combined_named_operands() {
    let (program, root) = shared_exponents(10);
    let child = program.render_expression(root);
    assert!(child.starts_with("evm.exp(base = "));
    assert!(child.len() < MAX_LIFTED_EXPRESSION_TEXT);
    assert!(child.len() * 2 > MAX_LIFTED_EXPRESSION_TEXT);
    let (_, ir, mut statement) = primitive(0x55, "SSTORE", 2);
    statement.inputs = vec![root, root];
    let original = (program.clone(), ir.clone(), statement.clone());
    assert_eq!(
        program
            .render_index(&ir)
            .render_statement(&statement, |_| true),
        Err(LiftedRenderError::Limit)
    );
    assert_eq!(
        program.render_statement(&statement, &ir),
        "statement_limit(s800);"
    );
    assert_eq!((program, ir, statement), original);
}

#[test]
fn invocation_names_and_punctuation_are_inside_the_exact_byte_ceiling() {
    let (program, ir, _) = primitive(0x00, "STOP", 0);
    let renderer = program.render_index(&ir);
    let exact_name = "x".repeat(MAX_LIFTED_EXPRESSION_TEXT - 6);
    let mut paid = 0;
    let text = renderer
        .render_invocation(0x00, &exact_name, &[], |units| {
            paid += units;
            true
        })
        .unwrap();
    assert_eq!(text.len(), MAX_LIFTED_EXPRESSION_TEXT);
    assert_eq!(text, format!("evm.{exact_name}()"));
    assert!(paid >= text.len());
    assert_eq!(
        renderer.render_invocation(0x00, &format!("{exact_name}x"), &[], |_| true),
        Err(LiftedRenderError::Limit)
    );
    assert_eq!(
        renderer.render_invocation(0x00, "stop", &[], |_| false),
        Err(LiftedRenderError::Limit)
    );
}
