use super::{Epoch, Equation, Error, analyze, call_shape, solve};
use crate::{
    Block, Edge, EdgeKind, Effects, EntityId, ExitKind, FactScope, Ir, Operation, Terminator, U256,
    Value, ValueFact, ValueKind,
};

pub(in crate::lifted::render::call_results) fn fixture(opcodes: &[u8]) -> (Ir, Vec<EntityId>) {
    let mut graph = Graph::new();
    let block = graph.block();
    let operations = opcodes
        .iter()
        .map(|opcode| graph.operation(block, *opcode))
        .collect();
    (graph.ir, operations)
}

struct Graph {
    ir: Ir,
    next: u64,
}
impl Graph {
    fn new() -> Self {
        Self {
            ir: Ir {
                fact_scope: FactScope::CompleteLocalCfg,
                ..Ir::default()
            },
            next: 1,
        }
    }
    fn id(&mut self) -> EntityId {
        let id = EntityId(self.next);
        self.next += 1;
        id
    }
    fn value(&mut self, kind: ValueKind) -> EntityId {
        let id = self.id();
        self.ir.values.push(Value {
            id,
            kind,
            fact: ValueFact::Unknown,
        });
        id
    }
    fn zero(&mut self) -> EntityId {
        self.value(ValueKind::Constant(U256::ZERO))
    }
    fn block(&mut self) -> usize {
        let id = self.id();
        let memory_parameter = self.value(ValueKind::MemoryParameter { block: id });
        let effect_parameter = self.value(ValueKind::EffectParameter { block: id });
        self.ir.entry.get_or_insert(id);
        self.ir.blocks.push(Block {
            id,
            source: None,
            parameters: Vec::new(),
            memory_parameter,
            effect_parameter,
            operations: Vec::new(),
            terminator: Terminator::Exit {
                kind: ExitKind::Stop,
                values: Vec::new(),
            },
        });
        self.ir.blocks.len() - 1
    }
    fn tokens(&self, block: usize) -> (EntityId, EntityId) {
        let block = &self.ir.blocks[block];
        (
            block
                .operations
                .iter()
                .rev()
                .find_map(|op| op.memory_output)
                .unwrap_or(block.memory_parameter),
            block
                .operations
                .iter()
                .rev()
                .find_map(|op| op.effect_output)
                .unwrap_or(block.effect_parameter),
        )
    }
    fn operation(&mut self, block: usize, opcode: u8) -> EntityId {
        let id = self.id();
        let (memory, effect) = self.tokens(block);
        let mutable = opcode != 0xfa;
        let (arity, results, effects) = match opcode {
            0xf1 | 0xf2 | 0xf4 | 0xfa => (
                if matches!(opcode, 0xf1 | 0xf2) { 7 } else { 6 },
                1,
                Effects {
                    memory_read: true,
                    memory_write: true,
                    storage_read: true,
                    storage_write: mutable,
                    transient_read: true,
                    transient_write: mutable,
                    world_read: true,
                    world_write: mutable,
                    call: true,
                    return_data_write: true,
                    ..Effects::default()
                },
            ),
            0xf0 | 0xf5 => (
                if opcode == 0xf0 { 3 } else { 4 },
                1,
                Effects {
                    memory_read: true,
                    storage_read: true,
                    storage_write: true,
                    transient_read: true,
                    transient_write: true,
                    world_read: true,
                    world_write: true,
                    create: true,
                    return_data_write: true,
                    ..Effects::default()
                },
            ),
            0x3d => (
                0,
                1,
                Effects {
                    return_data_read: true,
                    ..Effects::default()
                },
            ),
            0x3e => (
                3,
                0,
                Effects {
                    memory_write: true,
                    return_data_read: true,
                    ..Effects::default()
                },
            ),
            0x5a => (
                0,
                1,
                Effects {
                    world_read: true,
                    ..Effects::default()
                },
            ),
            _ => panic!("test primitive"),
        };
        let inputs = (0..arity).map(|_| self.zero()).collect();
        let outputs = (0..results)
            .map(|index| {
                self.value(ValueKind::Operation {
                    operation: id,
                    index,
                })
            })
            .collect();
        let memory_output = effects.memory_write.then(|| {
            self.value(ValueKind::Memory {
                operation: Some(id),
            })
        });
        let effect_output = self.value(ValueKind::Effect {
            operation: Some(id),
        });
        self.ir.blocks[block].operations.push(Operation {
            id,
            source: None,
            opcode,
            name: format!("op{opcode:02x}"),
            inputs,
            outputs,
            memory_input: (effects.memory_read || effects.memory_write).then_some(memory),
            memory_output,
            effects,
            replacement: None,
            effect_input: Some(effect),
            effect_output: Some(effect_output),
        });
        id
    }
    fn edge(&mut self, from: usize, to: usize, kind: EdgeKind) -> EntityId {
        let id = self.id();
        let (memory, effect) = self.tokens(from);
        self.ir.edges.push(Edge {
            id,
            from: self.ir.blocks[from].id,
            to: self.ir.blocks[to].id,
            kind,
            arguments: Vec::new(),
            memory,
            effect,
        });
        id
    }
    fn jump(&mut self, from: usize, to: usize) {
        let edge = self.edge(from, to, EdgeKind::Jump);
        let target = self.zero();
        self.ir.blocks[from].terminator = Terminator::Jump {
            target,
            edges: vec![edge],
            unresolved: false,
            invalid_targets: Vec::new(),
        };
    }
    fn branch(&mut self, from: usize, left: usize, right: usize) {
        let a = self.edge(from, left, EdgeKind::True);
        let b = self.edge(from, right, EdgeKind::False);
        let condition = self.zero();
        let target = self.zero();
        self.ir.blocks[from].terminator = Terminator::Branch {
            condition,
            target,
            then_edges: vec![a],
            else_edge: Some(b),
            unresolved: false,
            invalid_targets: Vec::new(),
        };
    }
    fn op_mut(&mut self, id: EntityId) -> &mut Operation {
        self.ir
            .blocks
            .iter_mut()
            .flat_map(|b| &mut b.operations)
            .find(|op| op.id == id)
            .unwrap()
    }
}

#[test]
fn all_call_roles_are_strict_and_create_is_only_an_epoch_reset() {
    let mut graph = Graph::new();
    let block = graph.block();
    for opcode in [0xf1, 0xf2, 0xf4, 0xfa] {
        let call = graph.operation(block, opcode);
        let size = graph.operation(block, 0x3d);
        let copy = graph.operation(block, 0x3e);
        let facts = analyze(&graph.ir, &mut |_| true).unwrap();
        assert!(call_shape(facts.operation(call).unwrap()));
        assert_eq!(facts.origin(size), Some(call));
        assert_eq!(facts.origin(copy), Some(call));
        let output = facts.operation(size).unwrap().outputs[0];
        assert_eq!(facts.word_origin(output), Some(call));
        let (_, position) = facts.position(call).unwrap();
        assert!(!facts.available(call, graph.ir.blocks[block].id, position));
        assert!(facts.available(call, graph.ir.blocks[block].id, position + 1));
    }
    for opcode in [0xf0, 0xf5] {
        let create = graph.operation(block, opcode);
        let size = graph.operation(block, 0x3d);
        let facts = analyze(&graph.ir, &mut |_| true).unwrap();
        assert!(!call_shape(facts.operation(create).unwrap()));
        assert_eq!(facts.origin(size), Some(create));
    }
}

#[test]
fn closed_diamond_preserves_one_call_but_conflicting_calls_do_not_merge() {
    for replace in [false, true] {
        let mut graph = Graph::new();
        let entry = graph.block();
        let left = graph.block();
        let right = graph.block();
        let join = graph.block();
        let call = graph.operation(entry, 0xf4);
        graph.operation(left, 0x5a);
        if replace {
            graph.operation(right, 0xfa);
        }
        let size = graph.operation(join, 0x3d);
        graph.branch(entry, left, right);
        graph.jump(left, join);
        graph.jump(right, join);
        let facts = analyze(&graph.ir, &mut |_| true).unwrap();
        assert_eq!(facts.origin(size), (!replace).then_some(call));
        assert!(facts.available(call, graph.ir.blocks[join].id, 0));
    }
    let mut graph = Graph::new();
    let entry = graph.block();
    let left = graph.block();
    let right = graph.block();
    let join = graph.block();
    let call = graph.operation(left, 0xf4);
    let read = graph.operation(join, 0x3d);
    graph.branch(entry, left, right);
    graph.jump(left, join);
    graph.jump(right, join);
    let facts = analyze(&graph.ir, &mut |_| true).unwrap();
    assert_eq!(facts.origin(read), None);
    assert!(!facts.available(call, graph.ir.blocks[join].id, 0));
}

#[test]
fn a_loop_keeps_an_anchored_buffer_and_accepts_a_fresh_local_call() {
    let mut graph = Graph::new();
    let entry = graph.block();
    let header = graph.block();
    let body = graph.block();
    let exit = graph.block();
    let outer = graph.operation(entry, 0xf4);
    let read = graph.operation(header, 0x3d);
    graph.operation(body, 0x5a);
    graph.jump(entry, header);
    graph.branch(header, body, exit);
    graph.jump(body, header);
    let facts = analyze(&graph.ir, &mut |_| true).unwrap();
    assert_eq!(facts.origin(read), Some(outer));

    let mut graph = Graph::new();
    let entry = graph.block();
    let header = graph.block();
    let exit = graph.block();
    let before = graph.operation(header, 0x3d);
    let fresh = graph.operation(header, 0xf4);
    let after = graph.operation(header, 0x3d);
    graph.jump(entry, header);
    graph.branch(header, header, exit);
    let facts = analyze(&graph.ir, &mut |_| true).unwrap();
    assert_eq!(facts.origin(before), None);
    assert_eq!(facts.origin(after), Some(fresh));
    assert!(facts.available(fresh, graph.ir.blocks[header].id, usize::MAX));
}

#[test]
fn unanchored_cycles_are_unknown_even_when_a_sibling_has_an_anchor() {
    for anchored in [false, true] {
        let equations = [
            Equation::Anchor(EntityId(1)),
            Equation::Join(vec![0, 2]),
            Equation::Join(vec![if anchored { 1 } else { 2 }]),
        ];
        let result = solve(&equations, &mut |_| true).unwrap();
        assert!(
            result[1]
                == if anchored {
                    Epoch::Call(EntityId(1))
                } else {
                    Epoch::Unknown
                }
        );
        assert!(result[2] == result[1]);
    }
}

#[test]
fn open_graphs_and_invalid_resets_do_not_expose_a_stale_buffer() {
    let mut graph = Graph::new();
    let entry = graph.block();
    let next = graph.block();
    let call = graph.operation(entry, 0xf4);
    let local = graph.operation(entry, 0x3d);
    let later = graph.operation(next, 0x3d);
    graph.jump(entry, next);
    if let Terminator::Jump { unresolved, .. } = &mut graph.ir.blocks[entry].terminator {
        *unresolved = true;
    }
    let facts = analyze(&graph.ir, &mut |_| true).unwrap();
    assert_eq!(facts.origin(local), Some(call));
    assert_eq!(facts.origin(later), None);
    assert!(!facts.available(call, graph.ir.blocks[next].id, 0));

    let mut graph = Graph::new();
    let block = graph.block();
    let first = graph.operation(block, 0xf4);
    let second = graph.operation(block, 0xf4);
    let read = graph.operation(block, 0x3d);
    let first_effect = graph.op_mut(first).effect_output;
    graph.op_mut(read).effect_input = first_effect;
    let facts = analyze(&graph.ir, &mut |_| true).unwrap();
    assert_eq!(
        facts.origin(read),
        Some(second),
        "raw order outranks a forged stale effect reference"
    );
    graph.op_mut(second).effects.return_data_write = false;
    let facts = analyze(&graph.ir, &mut |_| true).unwrap();
    assert_eq!(
        facts.origin(read),
        None,
        "a malformed CALL still invalidates the old epoch"
    );
    graph.op_mut(second).inputs.pop();
    assert!(!call_shape(graph.op_mut(second)));

    let mut graph = Graph::new();
    let block = graph.block();
    graph.operation(block, 0xf4);
    let unknown = graph.operation(block, 0x5a);
    graph.op_mut(unknown).effects.return_data_write = true;
    let read = graph.operation(block, 0x3d);
    let facts = analyze(&graph.ir, &mut |_| true).unwrap();
    assert_eq!(facts.origin(read), None);
}

#[test]
fn payment_refusal_is_an_error_and_never_an_unknown_success() {
    let mut graph = Graph::new();
    let block = graph.block();
    graph.operation(block, 0xf4);
    graph.operation(block, 0x3d);
    assert!(matches!(
        analyze(&graph.ir, &mut |_| false),
        Err(Error::Limit)
    ));
    let mut payments = 0;
    assert!(matches!(
        analyze(&graph.ir, &mut |_| {
            payments += 1;
            payments < 8
        }),
        Err(Error::Limit)
    ));
    assert_eq!(payments, 8);
}
