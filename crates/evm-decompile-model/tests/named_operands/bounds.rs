use super::super::{expression, primitive, words};
use evm_decompile_model::{
    EntityId, LiftedExpressionKind as Kind, LiftedProgram, LiftedRenderError, LiftedRenderIndex,
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
fn expression_size_limit_counts_nested_operators_and_delimiters() {
    let (program, root) = shared_exponents(13);
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
    let (program, root) = shared_exponents(12);
    let child = program.render_expression(root);
    assert_eq!(child.len(), 10 * (1 << 12) - 6);
    assert!(child.starts_with('(') && child.contains(" ** "));
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

fn render_operator(
    renderer: &LiftedRenderIndex<'_>,
    opcode: u8,
    arguments: &[EntityId],
    invocation: bool,
    pay: impl FnMut(usize) -> bool,
) -> Result<String, LiftedRenderError> {
    if invocation {
        renderer.render_invocation(opcode, "primitive", arguments, pay)
    } else {
        renderer.render_expression(super::super::ROOT, pay)
    }
}

#[test]
fn every_operator_payment_can_cancel_without_exposing_partial_text() {
    for (opcode, arity) in [
        (0x01, 2),
        (0x0a, 2),
        (0x10, 2),
        (0x15, 1),
        (0x19, 1),
        (0x1b, 2),
        (0x1c, 2),
    ] {
        let (mut program, ir, statement) = primitive(opcode, "PRIMITIVE", arity);
        program.expressions.push(expression(
            super::super::ROOT,
            Kind::WordOp {
                opcode,
                arguments: statement.inputs.clone(),
            },
        ));
        let renderer = program.render_index(&ir);
        for invocation in [false, true] {
            let mut payments = 0;
            render_operator(
                &renderer,
                opcode,
                &statement.inputs,
                invocation,
                |_: usize| {
                    payments += 1;
                    true
                },
            )
            .unwrap();
            for refuse_at in 0..payments {
                let mut visited = 0;
                let mut pay = |_: usize| {
                    let accept = visited != refuse_at;
                    visited += 1;
                    accept
                };
                assert_eq!(
                    render_operator(&renderer, opcode, &statement.inputs, invocation, &mut pay),
                    Err(LiftedRenderError::Limit),
                    "opcode={opcode:#x}, invocation={invocation}, payment={refuse_at}"
                );
            }
        }
    }
}

#[test]
fn deep_and_cyclic_operator_arenas_use_the_bounded_iterative_renderer() {
    let mut program = words(1);
    let depth = 10_000;
    for id in 2..=depth + 1 {
        program.expressions.push(expression(
            EntityId(id),
            Kind::WordOp {
                opcode: 0x19,
                arguments: vec![EntityId(id - 1)],
            },
        ));
    }
    program.validate().unwrap();
    let text = program.render_expression(EntityId(depth + 1));
    assert_eq!(text.len(), usize::try_from(depth).unwrap() * 3 + 4);
    assert!(text.starts_with("(~(~(~"));
    program.expressions[0].kind = Kind::WordOp {
        opcode: 0x19,
        arguments: vec![EntityId(1)],
    };
    let ir = Default::default();
    assert_eq!(
        program
            .render_index(&ir)
            .render_expression(EntityId(1), |_| true),
        Err(LiftedRenderError::Limit)
    );
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
