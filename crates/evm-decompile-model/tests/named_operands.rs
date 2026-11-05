//! Independent presentation-contract checks; expected labels never come from
//! the formatter's signature lookup or opcode metadata.
use evm_decompile_model::{
    Block, Effects, EntityId, Ir, LiftedExpression, LiftedExpressionKind as Kind, LiftedProgram,
    LiftedStatement, Operation, Terminator, U256,
};

mod named_operands {
    pub(super) mod bounds;
    pub(super) mod expressions;
    pub(super) mod statements;
}

const SENTINELS: [u64; 8] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
const ROOT: EntityId = EntityId(100);
const BLOCK: EntityId = EntityId(900);

fn expression(id: EntityId, kind: Kind) -> LiftedExpression {
    LiftedExpression {
        id,
        block: BLOCK,
        ty: kind.ty(),
        kind,
    }
}

fn words(count: usize) -> LiftedProgram {
    LiftedProgram {
        expressions: SENTINELS[..count]
            .iter()
            .enumerate()
            .map(|(index, value)| {
                expression(
                    EntityId(u64::try_from(index).unwrap() + 1),
                    Kind::Word(U256::from(*value)),
                )
            })
            .collect(),
        ..LiftedProgram::default()
    }
}

fn arguments(count: usize) -> Vec<EntityId> {
    (1..=u64::try_from(count).unwrap()).map(EntityId).collect()
}

// These intentionally admit malformed primitive arities: diagnostic formatting
// must preserve every supplied operand even when semantic admission would fail.
fn primitive(opcode: u8, name: &str, count: usize) -> (LiftedProgram, Ir, LiftedStatement) {
    let operation = Operation {
        id: EntityId(700),
        source: None,
        opcode,
        name: name.into(),
        // Distinct witness IDs detect accidentally formatting raw inputs.
        inputs: (1_001..=1_000 + u64::try_from(count).unwrap())
            .map(EntityId)
            .collect(),
        outputs: Vec::new(),
        effects: Effects::default(),
        memory_input: None,
        memory_output: None,
        replacement: None,
        effect_input: None,
        effect_output: None,
    };
    let statement = LiftedStatement {
        id: EntityId(800),
        operation: operation.id,
        inputs: arguments(count),
        outputs: Vec::new(),
    };
    let ir = Ir {
        entry: Some(BLOCK),
        blocks: vec![Block {
            id: BLOCK,
            source: None,
            parameters: Vec::new(),
            memory_parameter: EntityId(600),
            effect_parameter: EntityId(601),
            operations: vec![operation],
            terminator: Terminator::Pending,
        }],
        ..Ir::default()
    };
    (words(count), ir, statement)
}

type StatementCase = (u8, &'static str, usize, &'static str);

fn assert_statements(cases: &[StatementCase]) {
    for &(opcode, name, arity, expected) in cases {
        let (program, ir, statement) = primitive(opcode, name, arity);
        let original = (program.clone(), ir.clone(), statement.clone());
        let renderer = program.render_index(&ir);
        assert_eq!(
            program.render_statement(&statement, &ir),
            expected,
            "{name}"
        );
        assert_eq!(
            renderer
                .render_invocation(opcode, &name.to_lowercase(), &statement.inputs, |_| true)
                .unwrap(),
            expected.strip_suffix(';').unwrap(),
            "direct invocation for {name}"
        );
        for dependencies in [false, true] {
            let actual = renderer
                .render_statement_with_dependencies(&statement, dependencies, |_| true)
                .unwrap();
            assert_eq!(actual, expected, "{name}, dependencies={dependencies}");
            assert!(!actual.contains('\n'));
        }
        assert_eq!((program, ir, statement), original, "{name}");
    }
}

fn word_op(opcode: u8, count: usize) -> LiftedProgram {
    let mut program = words(count);
    program.expressions.push(expression(
        ROOT,
        Kind::WordOp {
            opcode,
            arguments: arguments(count),
        },
    ));
    program
}
