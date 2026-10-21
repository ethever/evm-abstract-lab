use super::*;
use crate::domain::{relational::RelationPathKind, symbolic::ExprKind};

fn selector(scalar: &EvmValue<ExcludingWord>) -> ExprId {
    let ExprKind::Ite {
        condition: selector,
        ..
    } = scalar.expression().unwrap().kind()
    else {
        panic!("changed whole-state scalar has no guarded join");
    };
    selector.clone()
}

#[test]
fn one_selector_pairs_continuations_current_store_checkpoints_and_relations() {
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (entry, mut left) = fixture(domain);
    let owner = left.active().key.address;
    let mut child = left.active().clone();
    child.key.caller = fixtures::address(0x202).into();
    left.call_stack.push_child(EvmChildFrame {
        state: child,
        continuation: EvmContinuation {
            return_block: Some(0),
            output_offset: value(23),
            output_size: value(29),
            creation: None,
        },
    });
    let mut right = left.clone();
    for frame in right.call_stack.iter_mut() {
        for scalar in frame.values_mut() {
            *scalar = value(13);
        }
        let mut saved = frame.saved_store.state().clone();
        saved.write_balance(owner, value(13));
        frame.saved_store = EvmSnapshot::from_state(saved);
    }
    let continuation = &mut right.call_stack.children_mut()[0].continuation;
    continuation.output_offset = value(23);
    continuation.output_size = value(29);
    right.store.write_balance(owner, value(13));
    let guard = ExprId::fresh().unwrap();
    let limits = domain.spec().relations();
    for (payload, word) in [(&mut left, U256::ZERO), (&mut right, U256::ONE)] {
        payload
            .relations
            .observe(&guard, &ExcludingWord::constant(word), &limits)
            .unwrap();
    }
    let joined = left.join(&right, domain).unwrap();
    let branch = selector(&joined.active().stack[0]);
    for frame in joined.call_stack.iter() {
        for scalar in frame.values() {
            assert_eq!(selector(scalar), branch);
            excludes(scalar, 13);
        }
        let saved_balance = frame.saved_store.state().read_balance(owner);
        assert_eq!(selector(&saved_balance), branch);
        excludes(&saved_balance, 13);
    }
    let current_balance = joined.store.read_balance(owner);
    assert_eq!(selector(&current_balance), branch);
    excludes(&current_balance, 13);
    let continuation = &joined.call_stack.children()[0].continuation;
    assert_eq!(selector(&continuation.output_offset), branch);
    assert_eq!(selector(&continuation.output_size), branch);
    excludes(&continuation.output_offset, 23);
    excludes(&continuation.output_size, 29);
    let RelationPathKind::Ite { selector, .. } = joined.relations.path().kind() else {
        panic!("different input constraints must stay under the whole-state selector");
    };
    assert_eq!(*selector, branch);

    // Canonical joins retain N while deliberately forgetting executable guards.
    let old = form_normalized(&left, &entry, domain).unwrap();
    let next = form_normalized(&right, &entry, domain).unwrap();
    for widen in [false, true] {
        let bound = join(&old, &next, domain, widen).unwrap();
        let replay = instantiate(&bound, domain).unwrap();
        for frame in replay.call_stack.iter() {
            frame.values().for_each(|scalar| excludes(scalar, 13));
            excludes(&frame.saved_store.state().read_balance(owner), 13);
        }
        excludes(&replay.store.read_balance(owner), 13);
        excludes(
            &replay.call_stack.children()[0].continuation.output_offset,
            23,
        );
    }
}

#[test]
fn malformed_templates_are_refused_before_join_or_materialization() {
    let algebra = ValueAlgebra::new(Algebra::new());
    let domain = algebra.borrowed();
    let (entry, input) = fixture(domain);
    let valid = form_normalized(&input, &entry, domain).unwrap();
    let mut malformed = valid.clone();
    malformed.frames.clear();
    assert_eq!(instantiate(&malformed, domain), Err(MachineError::Shape));
    assert_eq!(
        join(&malformed, &malformed, domain, false),
        Err(MachineError::Shape)
    );
    malformed = valid.clone();
    malformed.continuations.push(EvmContinuation {
        return_block: Some(0),
        output_offset: value(23),
        output_size: value(29),
        creation: None,
    });
    for widen in [false, true] {
        assert_eq!(
            join(&malformed, &valid, domain, widen),
            Err(MachineError::Shape)
        );
        assert_eq!(
            join(&valid, &malformed, domain, widen),
            Err(MachineError::Shape)
        );
    }
    assert!(semantic_eq(
        &valid,
        &form_normalized(&input, &entry, domain).unwrap()
    ));
}
