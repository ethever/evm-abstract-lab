//! Direct numeric and source admission checks, without projector reconstruction.
use super::*;
fn source() -> Source<'static, impl FnMut(usize) -> bool> {
    static IR: std::sync::OnceLock<Ir> = std::sync::OnceLock::new();
    Source::new(IR.get_or_init(Ir::default), |_| true).unwrap()
}
#[test]
fn ordinary_ordering_rejects_equal_and_reversed_overlapping_ranges() {
    let source = source();
    let state = State::default();
    for offset in 0_u64..180 {
        for target in 0_u64..180 {
            assert_eq!(
                source.disjoint(
                    &state,
                    &Expr::constant(U256::from(offset)),
                    &Expr::constant(U256::from(32)),
                    &Expr::constant(U256::from(target)),
                    32
                ),
                offset + 32 <= target || target + 32 <= offset,
                "write {offset}, cell {target}"
            );
        }
    }
}
#[test]
fn symbolic_relative_ordering_protects_headers_and_rejects_reversed_overlap() {
    let source = source();
    let n = Expr::atom(Atom::Source(EntityId(1)));
    let k = Expr::atom(Atom::Iteration(EntityId(2)));
    let mut state = State::default();
    state.atoms.insert(
        n.single().unwrap(),
        Range {
            low: U256::from(2),
            high: U256::from(1024),
        },
    );
    state.atoms.insert(
        k.single().unwrap(),
        Range {
            low: U256::ZERO,
            high: U256::from(1023),
        },
    );
    let header = n
        .scale(U256::from(48))
        .add(&Expr::constant(U256::from(224)));
    let fp = n
        .scale(U256::from(160))
        .add(&k.scale(U256::from(103)))
        .add(&Expr::constant(U256::from(288)));
    assert!(source.disjoint(&state, &fp, &Expr::constant(U256::from(96)), &header, 32));
    for delta in [U256::MAX, U256::ZERO, U256::from(31)] {
        let offset = header.add(&Expr::constant(delta));
        assert!(!source.disjoint(
            &state,
            &offset,
            &Expr::constant(U256::from(32)),
            &header,
            32
        ));
    }
    let offset = header.add(&Expr::constant(U256::from(32)));
    assert!(source.disjoint(
        &state,
        &offset,
        &Expr::constant(U256::from(96)),
        &header,
        32
    ));
}
#[test]
fn signed_remainder_inversion_preserves_a_feasible_n_minus_one_edge() {
    let mut source = source();
    let n = Expr::atom(Atom::Source(EntityId(3)));
    let mut state = State::default();
    state.atoms.insert(
        n.single().unwrap(),
        Range {
            low: U256::from(2),
            high: U256::from(10),
        },
    );
    let previous = state.atoms.clone();
    let minus_one = n.add(&Expr::constant(U256::MAX));
    assert_eq!(
        state.ordinary(&minus_one),
        Some(Range {
            low: U256::from(1),
            high: U256::from(9)
        })
    );
    assert!(source.restrict(
        &mut state,
        &minus_one,
        Range {
            low: U256::from(1),
            high: U256::from(9)
        },
        false
    ));
    assert_eq!(state.atoms, previous);
    assert!(source.restrict(
        &mut state,
        &minus_one,
        Range {
            low: U256::from(1),
            high: U256::from(7)
        },
        false
    ));
    assert_eq!(
        state.range(&n),
        Range {
            low: U256::from(2),
            high: U256::from(8)
        }
    );
}
#[test]
fn modular_sum_bounds_do_not_imply_an_ordinary_sum() {
    let mut source = source();
    let n = Expr::atom(Atom::Source(EntityId(4)));
    let mut state = State::default();
    let plus_one = n.add(&Expr::constant(U256::from(1)));
    assert!(source.restrict(
        &mut state,
        &plus_one,
        Range {
            low: U256::ZERO,
            high: U256::from(7)
        },
        false
    ));
    assert_eq!(state.range(&n), Range::FULL);
    assert!(state.ordinary(&plus_one).is_none());
}
#[test]
fn a_large_unsigned_multiplier_does_not_certify_a_negative_integer_expression() {
    let mut source = source();
    let n = Expr::atom(Atom::Source(EntityId(5)));
    let mut state = State::default();
    state.atoms.insert(
        n.single().unwrap(),
        Range {
            low: U256::ZERO,
            high: U256::from(1),
        },
    );
    let op = Operation {
        id: EntityId(6),
        source: None,
        opcode: 0x02,
        name: "MUL".into(),
        inputs: vec![EntityId(5); 2],
        outputs: vec![EntityId(7)],
        effects: crate::Effects::default(),
        memory_input: None,
        memory_output: None,
        replacement: None,
        effect_input: None,
        effect_output: None,
    };
    let expression = source
        .calculate(
            &op,
            &[n, Expr::constant(U256::MAX)],
            &mut state,
            Atom::Source(EntityId(7)),
        )
        .unwrap();
    assert!(state.ordinary(&expression).is_none());
    assert!(
        source.assume(&mut state, expression, true, false),
        "n=1 makes MAX*n nonzero; its feasible edge must survive"
    );
}
#[test]
fn signed_coefficient_carries_cannot_prove_nonaliasing() {
    let source = source();
    let x = Expr::atom(Atom::Source(EntityId(8)));
    let z = Expr::atom(Atom::Source(EntityId(9)));
    let c = U256::MAX >> 1;
    let w = c - U256::from(128);
    let mut state = State::default();
    state
        .atoms
        .insert(x.single().unwrap(), Range::exact(U256::from(1)));
    state
        .atoms
        .insert(z.single().unwrap(), Range::exact(U256::MAX - U256::from(2)));
    let cell = x.scale(c).sub(&Expr::constant(w));
    let write = z.sub(&x.scale(c)).sub(&Expr::constant(w));
    assert_eq!(state.ordinary(&cell), Some(Range::exact(U256::from(128))));
    assert_eq!(state.ordinary(&write), Some(Range::exact(U256::from(127))));
    assert!(!source.disjoint(&state, &write, &Expr::constant(U256::from(32)), &cell, 32));
    let cell = x.scale(c);
    let write = Expr::constant(c).sub(&cell);
    assert!(!source.disjoint(
        &state,
        &write,
        &Expr::constant(c + U256::from(1)),
        &cell,
        32
    ));
}
#[test]
fn singleton_ordering_matrix_agrees_with_concrete_modular_addresses() {
    let source = source();
    let atom = Atom::Source(EntityId(10));
    let half = U256::MAX >> 1;
    let samples = [
        U256::ZERO,
        U256::from(1),
        U256::from(127),
        U256::from(128),
        half,
        half + U256::from(1),
        U256::MAX,
    ];
    for value in samples {
        let mut state = State::default();
        state.atoms.insert(atom, Range::exact(value));
        for ac in samples {
            for bc in samples {
                for offset in [U256::ZERO, U256::from(127), U256::from(128), half] {
                    let a = Expr::atom(atom).scale(ac).add(&Expr::constant(offset));
                    let b = Expr::atom(atom).scale(bc).sub(&Expr::constant(offset));
                    let av = ac.wrapping_mul(value).wrapping_add(offset);
                    let bv = bc.wrapping_mul(value).wrapping_sub(offset);
                    if source.disjoint(&state, &a, &Expr::constant(U256::from(32)), &b, 32) {
                        assert!(
                            av.checked_add(U256::from(32)).is_some_and(|end| end <= bv)
                                || bv.checked_add(U256::from(32)).is_some_and(|end| end <= av),
                            "false separation at {av}/{bv}"
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn singleton_feasible_bound_matrix_never_discards_an_actual_assignment() {
    let mut source = source();
    let atom = Atom::Source(EntityId(11));
    let half = U256::MAX >> 1;
    let samples = [
        U256::ZERO,
        U256::from(1),
        U256::from(127),
        U256::from(128),
        half,
        half + U256::from(1),
        U256::MAX,
    ];
    for value in samples {
        for coefficient in samples {
            for constant in samples {
                let expr = Expr::atom(atom)
                    .scale(coefficient)
                    .add(&Expr::constant(constant));
                let actual = coefficient.wrapping_mul(value).wrapping_add(constant);
                let mut state = State::default();
                state.atoms.insert(atom, Range::exact(value));
                assert!(
                    source.restrict(&mut state, &expr, Range::exact(actual), false),
                    "discarded {value} * {coefficient} + {constant} = {actual}"
                );
                let after = state.atoms[&atom];
                assert!(after.low <= value && value <= after.high);
            }
        }
    }
}
#[test]
fn a_join_reserves_work_for_the_accumulated_left_operand() {
    let remaining = std::cell::Cell::new(usize::MAX);
    let block = Block {
        id: EntityId(1),
        source: None,
        parameters: vec![],
        memory_parameter: EntityId(2),
        effect_parameter: EntityId(3),
        operations: vec![],
        terminator: Terminator::Exit {
            kind: crate::ExitKind::Stop,
            values: vec![],
        },
    };
    let edge = |id, from| Edge {
        id: EntityId(id),
        from: EntityId(from),
        to: block.id,
        kind: EdgeKind::Jump,
        arguments: vec![],
        memory: EntityId(2),
        effect: EntityId(3),
    };
    let ir = Ir {
        blocks: vec![block.clone()],
        edges: vec![edge(6, 4), edge(7, 5)],
        ..Ir::default()
    };
    let mut source = Source::new(&ir, |cost| {
        if let Some(next) = remaining.get().checked_sub(cost) {
            remaining.set(next);
            true
        } else {
            false
        }
    })
    .unwrap();
    let mut large = State::default();
    for id in 100..164 {
        large
            .values
            .insert(EntityId(id), Expr::constant(U256::ZERO));
    }
    let flows = BTreeMap::from([
        (EntityId(6), Flow::Known(large)),
        (EntityId(7), Flow::Known(State::default())),
    ]);
    // Enough for both incoming clones, but not another scan of the accumulated
    // large state when the tiny second contribution is merged.
    remaining.set(64 * MAX_TERMS * MAX_TERMS + 6);
    assert!(matches!(source.join(&block, &flows), Flow::Unknown));
    assert!(source.refused);
    assert!(source.admitted.is_empty());
}
#[test]
fn ordinary_shift_cannot_change_the_signed_affine_representative() {
    let mut source = source();
    let half = U256::from(1) << 255;
    let x = Expr::atom(Atom::Source(EntityId(12)));
    let y = Expr::atom(Atom::Source(EntityId(13)));
    let mut state = State::default();
    state
        .atoms
        .insert(x.single().unwrap(), Range::exact(half - U256::from(1)));
    state.atoms.insert(y.single().unwrap(), Range::exact(half));
    let known = y.sub(&x).add(&Expr::constant(half - U256::from(3)));
    let exact = half - U256::from(2);
    assert_eq!(state.ordinary(&known), Some(Range::exact(exact)));
    state.real.insert(known.clone(), Range::exact(exact));
    let shifted = known.add(&Expr::constant(U256::from(10)));
    let actual = half + U256::from(8);
    assert!(state.ordinary(&shifted).is_none());
    assert!(source.restrict(&mut state, &shifted, Range::exact(actual), false));
}
#[test]
fn stale_folded_constant_cannot_hide_a_changed_source_opcode() {
    let operation = |id, opcode, inputs: Vec<EntityId>, output| Operation {
        id: EntityId(id),
        source: None,
        opcode,
        name: String::new(),
        inputs,
        outputs: vec![EntityId(output)],
        effects: crate::Effects::default(),
        memory_input: None,
        memory_output: None,
        replacement: None,
        effect_input: None,
        effect_output: None,
    };
    let block = Block {
        id: EntityId(1),
        source: None,
        parameters: vec![],
        memory_parameter: EntityId(2),
        effect_parameter: EntityId(3),
        operations: vec![
            operation(4, 0x60, vec![], 5),
            operation(6, 0x60, vec![], 7),
            operation(8, 0x02, vec![EntityId(5), EntityId(7)], 9),
        ],
        terminator: Terminator::Exit {
            kind: crate::ExitKind::EndOfCode,
            values: vec![],
        },
    };
    let literal = |id, value| crate::Value {
        id: EntityId(id),
        kind: ValueKind::Constant(U256::from(value)),
        fact: crate::ValueFact::Constants(vec![U256::from(value)]),
    };
    // Output9 retains the old ADD fold7, but its actual source is now MUL3*4.
    let ir = Ir {
        entry: Some(block.id),
        blocks: vec![block.clone()],
        values: vec![literal(5, 3_u64), literal(7, 4), literal(9, 7)],
        ..Ir::default()
    };
    let mut source = Source::new(&ir, |_| true).unwrap();
    let mut state = State {
        memory: Some(block.memory_parameter),
        effect: Some(block.effect_parameter),
        ..State::default()
    };
    assert!(source.execute(&block, &mut state, None, false));
    assert_eq!(
        source.value(&state, EntityId(9)).unwrap().exact(),
        Some(U256::from(12))
    );
}
