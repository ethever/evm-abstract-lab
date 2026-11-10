use super::*;
use crate::{Block, Cancellation, Config, Edge, EdgeKind, Fork, Input, artifact::Builder};

fn id(n: u64) -> EntityId {
    EntityId(n)
}

fn source(open: bool) -> Ir {
    let block = |n, terminator| Block {
        id: id(n),
        source: None,
        parameters: vec![],
        memory_parameter: id(1000 + n),
        effect_parameter: id(2000 + n),
        operations: vec![],
        terminator,
    };
    let branch = |left, right, unresolved| Terminator::Branch {
        condition: id(99),
        target: id(98),
        then_edges: vec![id(left)],
        else_edge: Some(id(right)),
        unresolved,
        invalid_targets: vec![],
    };
    Ir {
        entry: Some(id(10)),
        blocks: vec![
            block(10, branch(110, 111, false)),
            block(11, Terminator::Fallthrough { edge: id(112) }),
            block(12, branch(113, 114, false)),
            block(
                13,
                Terminator::Trap {
                    reason: "failure".into(),
                },
            ),
            block(1, branch(101, 102, false)),
            block(2, branch(103, 104, false)),
            block(3, Terminator::Fallthrough { edge: id(105) }),
            block(4, branch(106, 107, open)),
            block(
                5,
                Terminator::Exit {
                    kind: ExitKind::Revert,
                    values: vec![],
                },
            ),
            block(6, Terminator::Fallthrough { edge: id(108) }),
            block(8, Terminator::Fallthrough { edge: id(109) }),
            block(
                9,
                Terminator::Exit {
                    kind: ExitKind::Stop,
                    values: vec![],
                },
            ),
        ],
        edges: [
            (110, 10, 11, EdgeKind::True),
            (111, 10, 12, EdgeKind::False),
            (112, 11, 1, EdgeKind::Fallthrough),
            (113, 12, 13, EdgeKind::True),
            (114, 12, 1, EdgeKind::False),
            (101, 1, 2, EdgeKind::True),
            (102, 1, 8, EdgeKind::False),
            (103, 2, 3, EdgeKind::True),
            (104, 2, 4, EdgeKind::False),
            (105, 3, 6, EdgeKind::Fallthrough),
            (106, 4, 5, EdgeKind::True),
            (107, 4, 6, EdgeKind::False),
            (108, 6, 1, EdgeKind::Fallthrough),
            (109, 8, 9, EdgeKind::Fallthrough),
        ]
        .into_iter()
        .map(|(n, from, to, kind)| Edge {
            id: id(n),
            from: id(from),
            to: id(to),
            kind,
            arguments: vec![],
            memory: id(1000 + from),
            effect: id(2000 + from),
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

fn scope(stop: u64) -> RegionScope {
    RegionScope {
        continuation: Some(id(stop)),
        loop_header: Some(id(1)),
    }
}

#[test]
fn loop_normal_join_does_not_widen_global_finiteness_or_failures() {
    let ir = source(false);
    let facts = facts(&ir);
    assert!(!facts.finite.contains(&id(3)));
    assert!(!facts.finite.contains(&id(4)));
    assert_eq!(facts.failed, BTreeSet::from([id(5), id(13)]));
    let local = facts.normal_region(&ir, scope(1), &mut |_| true).unwrap();
    assert!(local.postdom[&id(3)].contains(&id(6)));
    assert!(local.postdom[&id(4)].contains(&id(6)));
    assert!(!local.postdom.contains_key(&id(5)));
    assert!(local.postdom[&id(1)].contains(&id(1)));
    assert!(!facts.normal_postdom.contains_key(&id(3)));
    assert!(!facts.finite.contains(&id(4)));
}

#[test]
fn prefixes_can_join_at_a_loop_entry_without_proving_that_loop_terminates() {
    let ir = source(false);
    let facts = facts(&ir);
    let local = facts
        .normal_region(
            &ir,
            RegionScope {
                continuation: None,
                loop_header: None,
            },
            &mut |_| true,
        )
        .unwrap();
    for arm in [11, 12] {
        assert!(!facts.finite.contains(&id(arm)));
        assert!(local.postdom[&id(arm)].contains(&id(1)));
        assert!(!local.postdom[&id(arm)].contains(&id(6)));
        assert!(!local.postdom[&id(arm)].contains(&id(8)));
    }
}

#[test]
fn stops_are_distinct_normal_boundaries_even_when_the_source_stop_reverts() {
    let ir = source(false);
    let facts = facts(&ir);
    let before_left = facts.normal_region(&ir, scope(3), &mut |_| true).unwrap();
    assert!(!before_left.postdom[&id(3)].contains(&id(6)));
    assert!(before_left.postdom[&id(4)].contains(&id(6)));
    let before_revert = facts.normal_region(&ir, scope(5), &mut |_| true).unwrap();
    assert!(facts.failed.contains(&id(5)));
    assert!(before_revert.postdom[&id(5)].contains(&id(5)));
    assert!(!before_revert.postdom[&id(4)].contains(&id(6)));
    // Continue and break boundaries never collapse to the same source anchor.
    let local = facts.normal_region(&ir, scope(1), &mut |_| true).unwrap();
    assert_eq!(
        local.postdom[&id(6)]
            .intersection(&local.postdom[&id(8)])
            .copied()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([id(0)])
    );
}

#[test]
fn unresolved_arms_and_budget_refusal_do_not_publish_partial_proofs() {
    let ir = source(true);
    let facts = facts(&ir);
    let local = facts.normal_region(&ir, scope(1), &mut |_| true).unwrap();
    assert!(!local.postdom.contains_key(&id(4)));
    assert!(!local.postdom.contains_key(&id(2)));
    for allowance in [0, 1, 4, 8] {
        let mut calls = 0;
        assert!(
            facts
                .normal_region(&ir, scope(1), &mut |_| {
                    calls += 1;
                    calls <= allowance
                })
                .is_none()
        );
    }
}
