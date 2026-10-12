use super::{Error, Kind, Solver, Status, Term};

#[test]
fn native_arrays_preserve_alias_conditions_write_order_and_previous_reads() {
    for alias in [true, false] {
        let mut solver = Solver::new(100_000).unwrap();
        let initial = solver.array_variable(256, 13, "storage").unwrap();
        let k = solver.variable(Some(256), "k").unwrap();
        let q = solver.variable(Some(256), "q").unwrap();
        let seven = solver.bv(13, "7").unwrap();
        let nine = solver.bv(13, "9").unwrap();
        let first = solver
            .apply(Kind::Store, &[initial, k, seven], &[])
            .unwrap();
        let history = solver.apply(Kind::Select, &[first, k], &[]).unwrap();
        let second = solver.apply(Kind::Store, &[first, q, nine], &[]).unwrap();
        let latest = solver.apply(Kind::Select, &[second, k], &[]).unwrap();
        let equality = solver.apply(Kind::Eq, &[k, q], &[]).unwrap();
        let guard = if alias {
            equality
        } else {
            solver.apply(Kind::Not, &[equality], &[]).unwrap()
        };
        solver.assert(&guard).unwrap();
        assert_eq!(solver.check().unwrap(), Status::Sat);
        assert_eq!(solver.model(&history).unwrap(), "#b0000000000111");
        assert_eq!(
            solver.model(&latest).unwrap(),
            if alias {
                "#b0000000001001"
            } else {
                "#b0000000000111"
            }
        );
        let expected = solver
            .apply(Kind::Ite, &[equality, nine, seven], &[])
            .unwrap();
        let matches = solver.apply(Kind::Eq, &[latest, expected], &[]).unwrap();
        let violation = solver.apply(Kind::Not, &[matches], &[]).unwrap();
        solver.assert(&violation).unwrap();
        assert_eq!(solver.check().unwrap(), Status::Unsat);
    }
}

#[test]
fn native_constant_arrays_accept_symbolic_defaults_and_array_conditionals() {
    let mut solver = Solver::new(100_000).unwrap();
    let default = solver.variable(Some(13), "default").unwrap();
    let original = solver.constant_array(257, &default).unwrap();
    let index = solver.variable(Some(257), "index").unwrap();
    let value = solver.bv(13, "1fff").unwrap();
    let updated = solver
        .apply(Kind::Store, &[original, index, value], &[])
        .unwrap();
    let flag = solver.variable(None, "flag").unwrap();
    let joined = solver
        .apply(Kind::Ite, &[flag, updated, original], &[])
        .unwrap();
    let read = solver.apply(Kind::Select, &[joined, index], &[]).unwrap();
    let expected = solver
        .apply(Kind::Ite, &[flag, value, default], &[])
        .unwrap();
    let equality = solver.apply(Kind::Eq, &[read, expected], &[]).unwrap();
    let violation = solver.apply(Kind::Not, &[equality], &[]).unwrap();
    solver.assert(&violation).unwrap();
    assert_eq!(solver.check().unwrap(), Status::Unsat);
}

#[test]
fn native_array_equality_is_extensional_and_array_models_are_rejected_without_poisoning() {
    let mut solver = Solver::new(100_000).unwrap();
    let a = solver.array_variable(8, 8, "a").unwrap();
    let b = solver.array_variable(8, 8, "b").unwrap();
    let index = solver.variable(Some(8), "index").unwrap();
    let first = solver.apply(Kind::Select, &[a, index], &[]).unwrap();
    let second = solver.apply(Kind::Select, &[b, index], &[]).unwrap();
    let equality = solver.apply(Kind::Eq, &[a, b], &[]).unwrap();
    solver.assert(&equality).unwrap();
    assert_eq!(solver.check().unwrap(), Status::Sat);
    assert!(solver.model(&a).is_err());
    assert_eq!(
        solver.model(&first).unwrap(),
        solver.model(&second).unwrap()
    );
    let equal_values = solver.apply(Kind::Eq, &[first, second], &[]).unwrap();
    let violation = solver.apply(Kind::Not, &[equal_values], &[]).unwrap();
    solver.assert(&violation).unwrap();
    assert_eq!(solver.check().unwrap(), Status::Unsat);
}

#[test]
fn array_width_sort_arity_indices_and_owners_are_validated_without_invalidating_session() {
    let mut solver = Solver::new(100_000).unwrap();
    assert!(solver.array_variable(0, 8, "a").is_err());
    assert!(solver.array_variable(8, 0, "a").is_err());
    assert!(solver.array_variable(8, 8, "bad\0name").is_err());
    let array = solver.array_variable(8, 13, "a").unwrap();
    let index = solver.bv(8, "0").unwrap();
    let wrong_index = solver.bv(9, "0").unwrap();
    let value = solver.bv(13, "1").unwrap();
    let boolean = solver.bool(true).unwrap();
    assert!(solver.constant_array(0, &value).is_err());
    assert!(solver.constant_array(8, &boolean).is_err());
    assert!(solver.constant_array(8, &array).is_err());
    for (kind, args, indices) in [
        (Kind::Select, vec![array], vec![]),
        (Kind::Select, vec![array, wrong_index], vec![]),
        (Kind::Select, vec![index, index], vec![]),
        (Kind::Select, vec![array, index], vec![0]),
        (Kind::Store, vec![array, index], vec![]),
        (Kind::Store, vec![array, index, index], vec![]),
        (Kind::Store, vec![array, wrong_index, value], vec![]),
        (Kind::Store, vec![array, index, boolean], vec![]),
    ] {
        assert!(solver.apply(kind, &args, &indices).is_err(), "{kind:?}");
    }
    let foreign = {
        let mut other = Solver::new(100).unwrap();
        other.bv(13, "0").unwrap()
    };
    assert_eq!(solver.constant_array(8, &foreign), Err(Error::CrossSession));
    assert_eq!(
        solver.apply(Kind::Store, &[array, index, foreign], &[]),
        Err(Error::CrossSession)
    );
    let forged = Term {
        owner: solver.owner,
        id: u64::MAX,
    };
    assert!(solver.constant_array(8, &forged).is_err());
    assert!(solver.apply(Kind::Select, &[forged, index], &[]).is_err());
    solver.assert(&boolean).unwrap();
    assert_eq!(solver.check().unwrap(), Status::Sat);
    assert_eq!(solver.model(&value).unwrap(), "#b0000000000001");
}

#[test]
fn native_array_queries_honor_resource_and_interrupt_signals() {
    for cancelled in [false, true] {
        let mut solver = Solver::new(if cancelled { 100_000 } else { 1 }).unwrap();
        let array = solver.array_variable(256, 8, "a").unwrap();
        let index = solver.variable(Some(256), "index").unwrap();
        let value = solver.apply(Kind::Select, &[array, index], &[]).unwrap();
        let square = solver.apply(Kind::Mul, &[value, value], &[]).unwrap();
        let four = solver.bv(8, "4").unwrap();
        let equality = solver.apply(Kind::Eq, &[square, four], &[]).unwrap();
        solver.assert(&equality).unwrap();
        if cancelled {
            solver.interrupt_handle().unwrap().interrupt();
        }
        assert_eq!(
            solver.check().unwrap(),
            Status::Unknown {
                exhausted: !cancelled,
                polls: u64::from(!cancelled)
            }
        );
        assert!(solver.model(&value).is_err());
    }
}

#[test]
fn native_constant_array_equality_unknown_is_not_resource_exhaustion_or_unsat() {
    let mut solver = Solver::new(100_000).unwrap();
    let default = solver.variable(Some(13), "default").unwrap();
    let zero = solver.bv(13, "0").unwrap();
    let one = solver.bv(13, "1").unwrap();
    let symbolic = solver.constant_array(256, &default).unwrap();
    let zeros = solver.constant_array(256, &zero).unwrap();
    let set_default = solver.apply(Kind::Eq, &[default, one], &[]).unwrap();
    let equality = solver.apply(Kind::Eq, &[symbolic, zeros], &[]).unwrap();
    solver.assert(&set_default).unwrap();
    solver.assert(&equality).unwrap();
    // The pinned native solver explicitly warns that this constant-array
    // equality is unsupported, rather than deciding the real contradiction.
    assert!(matches!(solver.check().unwrap(), Status::Unknown {
        exhausted: false,
        polls
    } if polls < 100_000));
    assert!(solver.model(&default).is_err());
}
