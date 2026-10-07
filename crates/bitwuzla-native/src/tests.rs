use super::{Error, Kind, Solver, Status, Term};

#[test]
fn sat_model_is_full_width_and_modular_arithmetic_wraps() {
    let mut solver = Solver::new(1_000_000).unwrap();
    let x = solver.variable(Some(256), "x").unwrap();
    let maximum = solver.bv(256, &"f".repeat(64)).unwrap();
    let one = solver.bv(256, "1").unwrap();
    let sum = solver.apply(Kind::Add, &[maximum, one], &[]).unwrap();
    let equality = solver.apply(Kind::Eq, &[x, sum], &[]).unwrap();
    solver.assert(&equality).unwrap();
    assert_eq!(solver.check().unwrap(), Status::Sat);
    assert_eq!(solver.model(&x).unwrap(), format!("#b{}", "0".repeat(256)));
}

#[test]
fn contradictory_assertions_are_unsat_and_models_are_invalidated() {
    let mut solver = Solver::new(1_000_000).unwrap();
    let x = solver.variable(Some(8), "x").unwrap();
    let zero = solver.bv(8, "00").unwrap();
    let one = solver.bv(8, "01").unwrap();
    let equal_zero = solver.apply(Kind::Eq, &[x, zero], &[]).unwrap();
    solver.assert(&equal_zero).unwrap();
    assert_eq!(solver.check().unwrap(), Status::Sat);
    assert_eq!(solver.model(&x).unwrap(), "#b00000000");
    let equal_one = solver.apply(Kind::Eq, &[x, one], &[]).unwrap();
    solver.assert(&equal_one).unwrap();
    assert!(solver.model(&x).is_err());
    assert_eq!(solver.check().unwrap(), Status::Unsat);
    assert!(solver.model(&x).is_err());
}

#[test]
fn boolean_variables_and_empty_connectives_have_correct_sorts() {
    let mut solver = Solver::new(1_000_000).unwrap();
    let p = solver.variable(None, "p").unwrap();
    let yes = solver.apply(Kind::And, &[], &[]).unwrap();
    let no = solver.apply(Kind::Or, &[], &[]).unwrap();
    let choice = solver.apply(Kind::Ite, &[p, yes, no], &[]).unwrap();
    solver.assert(&choice).unwrap();
    assert_eq!(solver.check().unwrap(), Status::Sat);
    assert_eq!(solver.model(&p).unwrap(), "#b1");
}

#[test]
fn terms_cannot_cross_solver_sessions_even_after_original_session_is_dropped() {
    let term = {
        let mut first = Solver::new(100).unwrap();
        first.bool(true).unwrap()
    };
    let mut second = Solver::new(100).unwrap();
    assert_eq!(second.assert(&term), Err(Error::CrossSession));
    assert_eq!(second.model(&term), Err(Error::CrossSession));
    assert_eq!(
        second.apply(Kind::Not, &[term], &[]),
        Err(Error::CrossSession)
    );
}

#[test]
fn invalid_width_hex_arity_sorts_and_indices_are_errors_not_native_aborts() {
    let mut solver = Solver::new(100).unwrap();
    assert!(Solver::new(0).is_err());
    assert!(solver.bv(0, "0").is_err());
    assert!(solver.bv(3, "f").is_err());
    assert!(solver.bv(8, "xz").is_err());
    assert!(solver.variable(Some(0), "x").is_err());
    assert!(solver.variable(None, "bad\0name").is_err());
    let x = solver.variable(Some(8), "x").unwrap();
    let y = solver.variable(Some(9), "y").unwrap();
    let p = solver.bool(true).unwrap();
    assert!(solver.apply(Kind::Add, &[x], &[]).is_err());
    assert!(solver.apply(Kind::Add, &[x, y], &[]).is_err());
    assert!(solver.apply(Kind::Not, &[x], &[]).is_err());
    assert!(solver.apply(Kind::Eq, &[x, p], &[]).is_err());
    assert!(solver.apply(Kind::Ite, &[x, p, p], &[]).is_err());
    assert!(solver.apply(Kind::Extract, &[x], &[8, 0]).is_err());
    assert!(solver.apply(Kind::Extract, &[x], &[2, 3]).is_err());
    assert!(solver.apply(Kind::Extract, &[x], &[]).is_err());
    assert!(solver.apply(Kind::ZeroExtend, &[x], &[u32::MAX]).is_err());
    assert!(solver.apply(Kind::BvNot, &[x], &[0]).is_err());
    assert!(solver.assert(&x).is_err());
    solver.assert(&p).unwrap();
    assert_eq!(solver.check().unwrap(), Status::Sat);
}

#[test]
fn forged_term_ids_are_checked_again_inside_the_native_boundary() {
    let mut solver = Solver::new(100).unwrap();
    let invalid = Term {
        owner: solver.owner,
        id: u64::MAX,
    };
    assert!(solver.assert(&invalid).is_err());
    assert!(solver.model(&invalid).is_err());
    assert!(solver.apply(Kind::Not, &[invalid], &[]).is_err());
    assert_eq!(solver.check().unwrap(), Status::Sat);
}

#[test]
fn tiny_poll_budget_returns_unknown_and_resets_for_each_check() {
    let mut solver = Solver::new(1).unwrap();
    let x = solver.variable(Some(256), "x").unwrap();
    let y = solver.variable(Some(256), "y").unwrap();
    let target = solver
        .bv(
            256,
            "fffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2f",
        )
        .unwrap();
    let product = solver.apply(Kind::Mul, &[x, y], &[]).unwrap();
    let equality = solver.apply(Kind::Eq, &[product, target], &[]).unwrap();
    solver.assert(&equality).unwrap();
    let one = solver.bv(256, "1").unwrap();
    for variable in [x, y] {
        let nontrivial = solver.apply(Kind::Ugt, &[variable, one], &[]).unwrap();
        solver.assert(&nontrivial).unwrap();
    }
    for _ in 0..2 {
        assert_eq!(
            solver.check().unwrap(),
            Status::Unknown {
                exhausted: true,
                polls: 1
            }
        );
        assert!(solver.model(&x).is_err());
    }
}

#[test]
fn primitive_mappings_preserve_signedness_shift_order_and_modular_width() {
    let mut solver = Solver::new(1_000_000).unwrap();
    // 0x81 is unsigned 129 and signed -127; 0x02 is positive two.
    let a = solver.bv(8, "81").unwrap();
    let b = solver.bv(8, "02").unwrap();
    let cases = [
        (Kind::Add, 0x83),
        (Kind::Sub, 0x7f),
        (Kind::Mul, 0x02),
        (Kind::Udiv, 0x40),
        (Kind::Sdiv, 0xc1),
        (Kind::Urem, 0x01),
        (Kind::Srem, 0xff),
        (Kind::Ult, 0),
        (Kind::Ule, 0),
        (Kind::Ugt, 1),
        (Kind::Uge, 1),
        (Kind::Slt, 1),
        (Kind::Sle, 1),
        (Kind::Sgt, 0),
        (Kind::Sge, 0),
        (Kind::BvAnd, 0x00),
        (Kind::BvOr, 0x83),
        (Kind::BvXor, 0x83),
        (Kind::Shl, 0x04),
        (Kind::Lshr, 0x20),
        (Kind::Ashr, 0xe0),
    ];
    let terms = cases
        .iter()
        .map(|(kind, _)| solver.apply(*kind, &[a, b], &[]).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(solver.check().unwrap(), Status::Sat);
    for ((kind, expected), term) in cases.iter().zip(&terms) {
        let model = solver.model(term).unwrap();
        assert_eq!(
            u8::from_str_radix(model.strip_prefix("#b").unwrap(), 2).unwrap(),
            *expected,
            "{kind:?}"
        );
    }
}

#[test]
fn unary_indexed_and_conditional_mappings_preserve_width_and_values() {
    let mut solver = Solver::new(1_000_000).unwrap();
    let byte = solver.bv(8, "81").unwrap();
    let complement = solver.apply(Kind::BvNot, &[byte], &[]).unwrap();
    let upper = solver.apply(Kind::Extract, &[byte], &[7, 4]).unwrap();
    let wide = solver.apply(Kind::ZeroExtend, &[byte], &[504]).unwrap();
    let yes = solver.bool(true).unwrap();
    let no = solver.apply(Kind::Not, &[yes], &[]).unwrap();
    let and = solver.apply(Kind::And, &[yes, no], &[]).unwrap();
    let or = solver.apply(Kind::Or, &[yes, no], &[]).unwrap();
    let choice = solver
        .apply(Kind::Ite, &[no, byte, complement], &[])
        .unwrap();
    assert_eq!(solver.check().unwrap(), Status::Sat);
    assert_eq!(solver.model(&upper).unwrap(), "#b1000");
    assert_eq!(
        solver.model(&wide).unwrap(),
        format!("#b{}10000001", "0".repeat(504))
    );
    assert_eq!(solver.model(&complement).unwrap(), "#b01111110");
    assert_eq!(solver.model(&choice).unwrap(), "#b01111110");
    assert_eq!(solver.model(&and).unwrap(), "#b0");
    assert_eq!(solver.model(&or).unwrap(), "#b1");
}
