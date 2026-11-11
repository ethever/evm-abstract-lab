//! Normal continuation proposals require a finite, closed source proof.
use super::*;
use crate::{Block, Cancellation, Config, Edge, EdgeKind, Fork, Input};
mod loops;
fn id(n: u64) -> EntityId {
    EntityId(n)
}
fn fixture(open: bool, cycle: bool) -> Ir {
    let block = |n, terminator| Block {
        id: id(n),
        source: None,
        parameters: vec![],
        memory_parameter: id(n + 10),
        effect_parameter: id(n + 20),
        operations: vec![],
        terminator,
    };
    Ir {
        entry: Some(id(1)),
        blocks: vec![
            block(
                1,
                Terminator::Branch {
                    condition: id(99),
                    target: id(98),
                    then_edges: vec![id(101)],
                    else_edge: Some(id(102)),
                    unresolved: false,
                    invalid_targets: vec![],
                },
            ),
            block(
                2,
                if open {
                    Terminator::Pending
                } else if cycle {
                    Terminator::Jump {
                        target: id(98),
                        edges: vec![id(103)],
                        unresolved: false,
                        invalid_targets: vec![],
                    }
                } else {
                    Terminator::Exit {
                        kind: ExitKind::Revert,
                        values: vec![],
                    }
                },
            ),
            block(
                3,
                Terminator::Exit {
                    kind: ExitKind::Stop,
                    values: vec![],
                },
            ),
        ],
        edges: [(101, 1, 2, EdgeKind::True), (102, 1, 3, EdgeKind::False)]
            .into_iter()
            .chain(cycle.then_some((103, 2, 2, EdgeKind::Jump)))
            .map(|(n, from, to, kind)| Edge {
                id: id(n),
                from: id(from),
                to: id(to),
                kind,
                arguments: vec![],
                memory: id(11),
                effect: id(21),
            })
            .collect(),
        ..Ir::default()
    }
}
#[test]
fn finite_failure_is_preserved_but_does_not_hide_the_normal_continuation() {
    let ir = fixture(false, false);
    let mut owner = Builder::new(
        &Input::new(vec![], Fork::Osaka),
        &Config::default(),
        &Cancellation::new(),
        |_| {},
    );
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    assert_eq!(facts.finite, BTreeSet::from([id(1), id(2), id(3)]));
    assert_eq!(facts.failed, BTreeSet::from([id(2)]));
    assert!(!facts.postdom[&id(1)].contains(&id(3)));
    assert!(facts.normal_postdom[&id(1)].contains(&id(3)));
}
#[test]
fn cycles_and_pending_successors_never_certify_a_finite_normal_continuation() {
    for (open, cycle) in [(true, false), (false, true)] {
        let ir = fixture(open, cycle);
        let mut owner = Builder::new(
            &Input::new(vec![], Fork::Osaka),
            &Config::default(),
            &Cancellation::new(),
            |_| {},
        );
        let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
        let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
        assert!(!facts.finite.contains(&id(1)));
        assert!(!facts.finite.contains(&id(2)));
        assert!(facts.failed.is_empty());
        assert!(!facts.normal_postdom.contains_key(&id(1)));
    }
}
