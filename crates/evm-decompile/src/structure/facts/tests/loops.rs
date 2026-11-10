//! Loop membership reuses finite failure proofs without widening them.
use super::*;

fn jump(edge: u64) -> Terminator {
    Terminator::Jump {
        target: id(99),
        edges: vec![id(edge)],
        unresolved: false,
        invalid_targets: vec![],
    }
}

fn branch(first: u64, second: u64) -> Terminator {
    Terminator::Branch {
        condition: id(99),
        target: id(98),
        then_edges: vec![id(first)],
        else_edge: Some(id(second)),
        unresolved: false,
        invalid_targets: vec![],
    }
}

fn terminal(kind: ExitKind) -> Terminator {
    Terminator::Exit {
        kind,
        values: vec![],
    }
}

fn loop_with_failure_dag() -> Ir {
    Ir {
        entry: Some(id(1)),
        blocks: vec![
            (1, jump(101)),
            (2, branch(102, 103)),
            (3, branch(104, 105)),
            (4, branch(106, 107)),
            (5, jump(108)),
            (6, jump(109)),
            (7, jump(110)),
            (8, jump(111)),
            (9, terminal(ExitKind::Stop)),
            (10, terminal(ExitKind::Revert)),
        ]
        .into_iter()
        .map(|(n, terminator)| Block {
            id: id(n),
            source: None,
            parameters: vec![],
            memory_parameter: id(n + 20),
            effect_parameter: id(n + 40),
            operations: vec![],
            terminator,
        })
        .collect(),
        edges: [
            (101, 1, 2, EdgeKind::Jump),
            (102, 2, 3, EdgeKind::True),
            (103, 2, 8, EdgeKind::False),
            (104, 3, 4, EdgeKind::True),
            (105, 3, 5, EdgeKind::False),
            (106, 4, 6, EdgeKind::True),
            (107, 4, 7, EdgeKind::False),
            (108, 5, 2, EdgeKind::Jump),
            (109, 6, 10, EdgeKind::Jump),
            (110, 7, 10, EdgeKind::Jump),
            (111, 8, 9, EdgeKind::Jump),
        ]
        .into_iter()
        .map(|(n, from, to, kind)| Edge {
            id: id(n),
            from: id(from),
            to: id(to),
            kind,
            arguments: vec![],
            memory: id(21),
            effect: id(41),
        })
        .collect(),
        ..Ir::default()
    }
}

fn facts(ir: &Ir) -> Facts {
    let mut owner = Builder::new(
        &Input::new(vec![], Fork::Osaka),
        &Config::default(),
        &Cancellation::new(),
        |_| {},
    );
    let dom = crate::transform::dominators(ir, &mut owner).unwrap();
    Facts::new(ir, &dom, &mut owner).unwrap()
}

#[test]
fn complete_failure_dag_joins_the_body_but_normal_suffix_remains_a_follow() {
    let ir = loop_with_failure_dag();
    let facts = facts(&ir);
    let lp = &facts.loops[&id(2)];
    assert_eq!(facts.failed, BTreeSet::from([id(4), id(6), id(7), id(10)]));
    assert_eq!(
        lp.nodes,
        BTreeSet::from([id(2), id(3), id(4), id(5), id(6), id(7), id(10)])
    );
    assert_eq!(lp.exit, Some(id(8)));
    assert!(facts.finite.contains(&id(8)));
    assert!(!facts.finite.contains(&id(2)));
}

#[test]
fn single_failure_suffix_stays_outside_an_already_admitted_loop() {
    let mut ir = loop_with_failure_dag();
    ir.blocks
        .iter_mut()
        .find(|b| b.id == id(3))
        .unwrap()
        .terminator = jump(105);
    ir.edges.retain(|edge| edge.id != id(104));
    ir.blocks
        .iter_mut()
        .find(|b| b.id == id(9))
        .unwrap()
        .terminator = terminal(ExitKind::Revert);
    let facts = facts(&ir);
    let lp = &facts.loops[&id(2)];
    assert_eq!(lp.nodes, BTreeSet::from([id(2), id(3), id(5)]));
    assert_eq!(lp.exit, Some(id(8)));
    assert!(facts.failed.contains(&id(8)));
}

#[test]
fn multiple_failure_bridges_keep_their_existing_shared_follow() {
    let mut ir = loop_with_failure_dag();
    for edge in ir
        .edges
        .iter_mut()
        .filter(|edge| [id(109), id(110)].contains(&edge.id))
    {
        edge.to = id(9);
    }
    ir.blocks
        .iter_mut()
        .find(|b| b.id == id(9))
        .unwrap()
        .terminator = jump(112);
    ir.edges.push(Edge {
        id: id(112),
        from: id(9),
        to: id(10),
        kind: EdgeKind::Jump,
        arguments: vec![],
        memory: id(21),
        effect: id(41),
    });
    let facts = facts(&ir);
    let lp = &facts.loops[&id(2)];
    assert_eq!(
        lp.nodes,
        BTreeSet::from([id(2), id(3), id(4), id(5), id(6), id(7), id(8)])
    );
    assert_eq!(lp.exit, Some(id(9)));
    assert!(facts.failed.contains(&id(9)));
    assert!(!lp.nodes.contains(&id(10)));
}

#[test]
fn direct_normal_terminal_exits_keep_the_existing_body_membership() {
    let mut ir = loop_with_failure_dag();
    ir.blocks
        .iter_mut()
        .find(|b| b.id == id(8))
        .unwrap()
        .terminator = terminal(ExitKind::Return);
    ir.edges.retain(|edge| edge.id != id(111));
    let facts = facts(&ir);
    let lp = &facts.loops[&id(2)];
    assert!(lp.nodes.contains(&id(8)));
    assert_eq!(lp.exit, Some(id(4)));
}

#[test]
fn pending_unknown_invalid_and_cyclic_failure_alternatives_do_not_widen_the_proof() {
    for terminator in [
        Terminator::Pending,
        Terminator::Jump {
            target: id(99),
            edges: vec![id(110)],
            unresolved: true,
            invalid_targets: vec![],
        },
        Terminator::Jump {
            target: id(99),
            edges: vec![id(110)],
            unresolved: false,
            invalid_targets: vec![crate::U256::from(1)],
        },
        jump(110),
    ] {
        let cycle = matches!(
            &terminator,
            Terminator::Jump {
                unresolved: false,
                invalid_targets,
                ..
            } if invalid_targets.is_empty()
        );
        let pending = matches!(&terminator, Terminator::Pending);
        let mut ir = loop_with_failure_dag();
        ir.blocks
            .iter_mut()
            .find(|b| b.id == id(7))
            .unwrap()
            .terminator = terminator;
        if cycle {
            ir.edges.iter_mut().find(|e| e.id == id(110)).unwrap().to = id(7);
        } else if pending {
            ir.edges.retain(|edge| edge.id != id(110));
        }
        let facts = facts(&ir);
        assert!(!facts.failed.contains(&id(4)));
        assert!(!facts.failed.contains(&id(7)));
        assert!(!facts.finite.contains(&id(4)));
        assert!(!facts.loops.contains_key(&id(2)));
    }
}
