//! Independent source/candidate mutations, included as an induction child test
//! module. Only original discovery creates a candidate; corrupted source goes
//! straight through fresh entry replay and the supplied-candidate verifier.
use super::super::{Atom, Predicate, Range};
use super::{Candidate, Expr, Flow, Source, State};
use crate::{
    Config, EntityId, Fork, Input, Ir, StageKind, Terminator, U256, Value, ValueFact, ValueKind,
    decompile,
};
use std::collections::BTreeMap;

#[path = "../fixtures/asm.rs"]
#[expect(
    dead_code,
    reason = "shared integration assembler also supports data and fragment fixtures"
)]
pub(super) mod assembly;
use assembly::Asm;

fn original() -> Ir {
    witness(false)
}

fn second_loop(asm: &mut Asm) {
    asm.number(0)
        .target("head2")
        .op(0x56)
        .dest("head2")
        .op(0x80)
        .number(128)
        .op(0x51)
        .op(0x90)
        .op(0x10)
        .target("body2")
        .op(0x57)
        .op(0x50)
        .number(64)
        .op(0x51)
        .op(0x50)
        .op(0x00)
        .dest("body2")
        .number(64)
        .op(0x51)
        .op(0x80)
        .number(5)
        .op(0x01)
        .number(64)
        .op(0x52)
        .number(19)
        .op(0x01)
        .number(64)
        .op(0x52)
        .number(96)
        .number(64)
        .op(0x51)
        .number(0)
        .number(0)
        .number(1)
        .number(255)
        .op(0xfa)
        .op(0x50)
        .number(1)
        .op(0x01)
        .target("head2")
        .op(0x56);
}

fn witness(chained: bool) -> Ir {
    let artifact = decompile(&byte_input(chained), &Config::default());
    artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Structured)
        .expect("original source must reach the unchanged Structured witness")
        .ir
        .clone()
}

fn byte_input(chained: bool) -> Input {
    let mut asm = Asm::default();
    asm.number(0)
        .op(0x35)
        .op(0x80)
        .number(7)
        .op(0x90)
        .op(0x11)
        .target("reject")
        .op(0x57)
        .op(0x80)
        .number(128)
        .op(0x52)
        .number(13)
        .op(0x02)
        .number(512)
        .op(0x01)
        .number(64)
        .op(0x52)
        .number(0)
        .target("head")
        .op(0x56)
        .dest("head")
        .op(0x80)
        .number(128)
        .op(0x51)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50);
    if chained {
        second_loop(&mut asm);
    } else {
        asm.number(64).op(0x51).op(0x50).op(0x00);
    }
    asm.dest("body")
        .number(0x42)
        .op(0x81)
        .number(32)
        .op(0x02)
        .number(160)
        .op(0x01)
        .op(0x52)
        .number(64)
        .op(0x51)
        .op(0x80)
        .number(17)
        .op(0x01)
        .number(64)
        .op(0x52)
        .number(37)
        .op(0x01)
        .number(64)
        .op(0x52)
        .number(96)
        .number(64)
        .op(0x51)
        .number(0)
        .number(0)
        .number(1)
        .number(255)
        .op(0xfa)
        .op(0x50)
        .number(1)
        .op(0x01)
        .target("head")
        .op(0x56)
        .dest("reject")
        .op(0x00);
    asm.finish()
}

fn components(ir: &Ir) -> Vec<Vec<EntityId>> {
    super::super::super::cycles::components(ir, &mut |_| true).unwrap()
}

fn entries<P: FnMut(usize) -> bool>(
    source: &mut Source<'_, P>,
    component: &[EntityId],
) -> BTreeMap<EntityId, Flow> {
    let mut flows = BTreeMap::new();
    for group in components(source.ir) {
        if group.iter().any(|id| component.contains(id)) {
            break;
        }
        assert_eq!(
            group.len(),
            1,
            "the independent fixture has one cyclic region"
        );
        let block = source.blocks[&group[0]];
        let flow = if Some(block.id) == source.ir.entry {
            Flow::Known(State {
                memory: Some(block.memory_parameter),
                effect: Some(block.effect_parameter),
                ..State::default()
            })
        } else {
            source.join(block, &flows)
        };
        source.process(block, flow, None, false, &mut flows, false);
    }
    flows
}

fn candidate(ir: &Ir) -> (Vec<EntityId>, Candidate) {
    let component = components(ir)
        .into_iter()
        .find(|group| group.len() > 1)
        .unwrap();
    let mut source = Source::new(ir, |_| true).unwrap();
    let incoming = entries(&mut source, &component);
    let shape = source.shape(&component).unwrap();
    let initial = source.entry_state(&shape, &incoming).unwrap();
    let proposal = source
        .propose(&shape, &initial)
        .expect("original source must discover a bounded recurrence");
    (component, proposal)
}

fn accepts(ir: &Ir, component: &[EntityId], candidate: &Candidate) -> bool {
    let mut verifier = Source::new(ir, |_| true).unwrap();
    let incoming = entries(&mut verifier, component);
    verifier
        .verify_candidate(component, candidate, &incoming)
        .is_some()
}

#[derive(Clone, Copy, Debug)]
enum CandidateChange {
    Header,
    Bound,
    ScalarInitial,
    ScalarStep,
    CellInitial,
    CellStep,
    ScalarRange,
    CellRange,
    CellAddress,
}

#[test]
fn independent_supplied_candidate_verifier_rejects_forged_recurrence_fields() {
    let ir = original();
    let (component, original) = candidate(&ir);
    assert!(accepts(&ir, &component, &original));
    for change in [
        CandidateChange::Header,
        CandidateChange::Bound,
        CandidateChange::ScalarInitial,
        CandidateChange::ScalarStep,
        CandidateChange::CellInitial,
        CandidateChange::CellStep,
        CandidateChange::ScalarRange,
        CandidateChange::CellRange,
        CandidateChange::CellAddress,
    ] {
        let mut wrong = original.clone();
        match change {
            CandidateChange::Header => wrong.header = ir.entry.unwrap(),
            CandidateChange::Bound => wrong.bound = U256::ZERO,
            CandidateChange::ScalarInitial => {
                wrong.scalars.values_mut().next().unwrap().initial = Expr::constant(U256::MAX)
            }
            CandidateChange::ScalarStep => {
                let recurrence = wrong
                    .scalars
                    .values_mut()
                    .find(|r| r.step != U256::ZERO)
                    .unwrap();
                recurrence.step = recurrence.step.wrapping_add(U256::from(1));
            }
            CandidateChange::CellInitial => {
                wrong.cells.values_mut().next().unwrap().initial = Expr::constant(U256::MAX)
            }
            CandidateChange::CellStep => {
                let recurrence = wrong
                    .cells
                    .values_mut()
                    .find(|r| r.step != U256::ZERO)
                    .unwrap();
                recurrence.step = recurrence.step.wrapping_add(U256::from(1));
            }
            CandidateChange::ScalarRange => {
                wrong.scalars.values_mut().next().unwrap().range = Range::exact(U256::MAX);
            }
            CandidateChange::CellRange => {
                wrong.cells.values_mut().next().unwrap().range = Range::exact(U256::MAX);
            }
            CandidateChange::CellAddress => {
                let (address, recurrence) = wrong.cells.pop_first().unwrap();
                wrong
                    .cells
                    .insert(address.add(&Expr::constant(U256::from(1))), recurrence);
            }
        }
        assert!(
            !accepts(&ir, &component, &wrong),
            "accepted forged {change:?}"
        );
    }
}

fn constant(ir: &Ir, id: EntityId) -> Option<U256> {
    match ir.value(id)?.kind {
        ValueKind::Constant(value) => Some(value),
        _ => None,
    }
}

fn add_constant(ir: &mut Ir, value: u64) -> EntityId {
    let id = EntityId(u64::MAX - 10_000 + u64::try_from(ir.values.len()).unwrap());
    ir.values.push(Value {
        id,
        kind: ValueKind::Constant(U256::from(value)),
        fact: ValueFact::Constants(vec![U256::from(value)]),
    });
    id
}

#[derive(Clone, Copy, Debug)]
enum SourceChange {
    StepOpcode,
    StepArgument,
    Effect,
    StoreArity,
    LoadArity,
    CallArity,
    Producer,
    ReadMemory,
    WriteMemory,
    EdgeMemory,
    EdgeArguments,
    HeaderAlias,
    ReverseHeaderOverlap,
    ReverseSlotOverlap,
    CallOutputAlias,
    CounterNoProgress,
    EntryBound,
    HeaderInitialization,
    ExternalInitializer,
    ExtraEntry,
    ExtraBackedge,
    ParameterSlot,
    UnresolvedPrefix,
    UnresolvedBackedge,
}

fn corrupt(ir: &mut Ir, component: &[EntityId], proposal: &Candidate, change: SourceChange) {
    let slot = add_constant(ir, 64);
    let header = add_constant(ir, 128);
    let header_minus_one = add_constant(ir, 127);
    let slot_minus_one = add_constant(ir, 63);
    let zero = add_constant(ir, 0);
    let forged = EntityId(u64::MAX);
    let inside = |block: &crate::Block| component.contains(&block.id);
    let indexed = ir
        .blocks
        .iter()
        .filter(|b| inside(b))
        .flat_map(|b| &b.operations)
        .find(|op| op.opcode == 0x52 && constant(ir, op.inputs[0]).is_none())
        .unwrap()
        .id;
    let step = ir
        .blocks
        .iter()
        .filter(|b| inside(b))
        .flat_map(|b| &b.operations)
        .find(|op| {
            op.opcode == 0x01
                && op
                    .inputs
                    .iter()
                    .any(|id| constant(ir, *id) == Some(U256::from(37)))
        })
        .unwrap()
        .id;
    let counter = ir
        .blocks
        .iter()
        .filter(|b| inside(b))
        .flat_map(|b| &b.operations)
        .find(|op| {
            op.opcode == 0x01
                && op
                    .inputs
                    .iter()
                    .any(|id| constant(ir, *id) == Some(U256::from(1)))
        })
        .unwrap()
        .id;
    let load = ir
        .blocks
        .iter()
        .filter(|b| inside(b))
        .flat_map(|b| &b.operations)
        .find(|op| op.opcode == 0x51)
        .unwrap()
        .id;
    let call = ir
        .blocks
        .iter()
        .filter(|b| inside(b))
        .flat_map(|b| &b.operations)
        .find(|op| op.opcode == 0xfa)
        .unwrap()
        .id;
    let initial_slot = ir
        .blocks
        .iter()
        .filter(|b| !inside(b))
        .flat_map(|b| &b.operations)
        .find(|op| op.opcode == 0x52 && constant(ir, op.inputs[0]) == Some(U256::from(64)))
        .unwrap()
        .id;
    let initial_header = ir
        .blocks
        .iter()
        .filter(|b| !inside(b))
        .flat_map(|b| &b.operations)
        .find(|op| op.opcode == 0x52 && constant(ir, op.inputs[0]) == Some(U256::from(128)))
        .unwrap()
        .id;
    let bound = ir
        .blocks
        .iter()
        .filter(|b| !inside(b))
        .flat_map(|b| &b.operations)
        .find(|op| op.opcode == 0x11)
        .unwrap()
        .id;
    let backedge = ir
        .edges
        .iter()
        .find(|edge| component.contains(&edge.from) && edge.to == proposal.header)
        .unwrap()
        .id;
    let entry = ir
        .edges
        .iter()
        .find(|edge| !component.contains(&edge.from) && edge.to == proposal.header)
        .unwrap()
        .id;
    match change {
        SourceChange::Producer => {
            let output = ir
                .blocks
                .iter()
                .flat_map(|b| &b.operations)
                .find(|op| op.id == load)
                .unwrap()
                .outputs[0];
            ir.values
                .iter_mut()
                .find(|value| value.id == output)
                .unwrap()
                .kind = ValueKind::Operation {
                operation: forged,
                index: 0,
            };
        }
        SourceChange::EdgeMemory => {
            ir.edges
                .iter_mut()
                .find(|edge| edge.id == backedge)
                .unwrap()
                .memory = forged
        }
        SourceChange::EdgeArguments => {
            let scalar = *proposal
                .scalars
                .iter()
                .find(|(_, recurrence)| recurrence.step != U256::ZERO)
                .unwrap()
                .0;
            let index = ir
                .block(proposal.header)
                .unwrap()
                .parameters
                .iter()
                .position(|id| *id == scalar)
                .unwrap();
            ir.edges
                .iter_mut()
                .find(|edge| edge.id == backedge)
                .unwrap()
                .arguments[index] = zero;
        }
        SourceChange::ParameterSlot => {
            let scalar = *proposal.scalars.keys().next().unwrap();
            let value = ir
                .values
                .iter_mut()
                .find(|value| value.id == scalar)
                .unwrap();
            let ValueKind::Parameter { index, .. } = &mut value.kind else {
                panic!("header scalar is a phi")
            };
            *index += 1;
        }
        SourceChange::UnresolvedPrefix | SourceChange::UnresolvedBackedge => {
            let edge = ir
                .edges
                .iter()
                .find(|edge| {
                    edge.id
                        == if matches!(change, SourceChange::UnresolvedPrefix) {
                            entry
                        } else {
                            backedge
                        }
                })
                .unwrap();
            let from = edge.from;
            let block = ir.blocks.iter_mut().find(|block| block.id == from).unwrap();
            let Terminator::Jump { unresolved, .. } = &mut block.terminator else {
                panic!("fixture has explicit source jumps")
            };
            *unresolved = true;
        }
        SourceChange::ExtraEntry | SourceChange::ExtraBackedge => {
            let mut edge = ir
                .edges
                .iter()
                .find(|edge| {
                    edge.id
                        == if matches!(change, SourceChange::ExtraEntry) {
                            entry
                        } else {
                            backedge
                        }
                })
                .unwrap()
                .clone();
            edge.id = EntityId(u64::MAX - 1);
            if matches!(change, SourceChange::ExtraEntry) {
                edge.to = *component.iter().find(|id| **id != proposal.header).unwrap();
                edge.arguments = vec![zero; ir.block(edge.to).unwrap().parameters.len()];
            } else {
                edge.arguments.fill(zero);
            }
            let block = ir
                .blocks
                .iter_mut()
                .find(|block| block.id == edge.from)
                .unwrap();
            let Terminator::Jump { edges, .. } = &mut block.terminator else {
                panic!("fixture has explicit source jumps")
            };
            edges.push(edge.id);
            ir.edges.push(edge);
        }
        _ => {
            let id = match change {
                SourceChange::StepOpcode | SourceChange::StepArgument => step,
                SourceChange::CounterNoProgress => counter,
                SourceChange::LoadArity | SourceChange::ReadMemory => load,
                SourceChange::CallArity | SourceChange::CallOutputAlias => call,
                SourceChange::EntryBound => bound,
                SourceChange::HeaderInitialization => initial_header,
                SourceChange::ExternalInitializer => initial_slot,
                _ => indexed,
            };
            let op = ir
                .blocks
                .iter_mut()
                .flat_map(|block| &mut block.operations)
                .find(|op| op.id == id)
                .unwrap();
            match change {
                SourceChange::StepOpcode => op.opcode = 0x02,
                SourceChange::StepArgument | SourceChange::CounterNoProgress => op.inputs[0] = zero,
                SourceChange::Effect => op.effects.memory_write = false,
                SourceChange::StoreArity | SourceChange::LoadArity | SourceChange::CallArity => {
                    op.inputs.pop();
                }
                SourceChange::ReadMemory => op.memory_input = Some(forged),
                SourceChange::WriteMemory => op.memory_output = Some(forged),
                SourceChange::HeaderAlias => op.inputs[0] = header,
                SourceChange::ReverseHeaderOverlap => op.inputs[0] = header_minus_one,
                SourceChange::ReverseSlotOverlap => op.inputs[0] = slot_minus_one,
                SourceChange::CallOutputAlias => op.inputs[4] = slot,
                SourceChange::EntryBound => op.opcode = 0x10,
                SourceChange::HeaderInitialization | SourceChange::ExternalInitializer => {
                    op.inputs[1] = zero
                }
                _ => unreachable!(),
            }
        }
    }
}

#[test]
fn independent_source_replay_rejects_forged_producers_entries_steps_headers_and_ranges() {
    let ir = original();
    let (component, proposal) = candidate(&ir);
    assert!(accepts(&ir, &component, &proposal));
    for change in [
        SourceChange::StepOpcode,
        SourceChange::StepArgument,
        SourceChange::Effect,
        SourceChange::StoreArity,
        SourceChange::LoadArity,
        SourceChange::CallArity,
        SourceChange::Producer,
        SourceChange::ReadMemory,
        SourceChange::WriteMemory,
        SourceChange::EdgeMemory,
        SourceChange::EdgeArguments,
        SourceChange::HeaderAlias,
        SourceChange::ReverseHeaderOverlap,
        SourceChange::ReverseSlotOverlap,
        SourceChange::CallOutputAlias,
        SourceChange::CounterNoProgress,
        SourceChange::EntryBound,
        SourceChange::HeaderInitialization,
        SourceChange::ExternalInitializer,
        SourceChange::ExtraEntry,
        SourceChange::ExtraBackedge,
        SourceChange::ParameterSlot,
        SourceChange::UnresolvedPrefix,
        SourceChange::UnresolvedBackedge,
    ] {
        let mut wrong = ir.clone();
        corrupt(&mut wrong, &component, &proposal, change);
        assert!(
            !accepts(&wrong, &component, &proposal),
            "accepted corrupted actual source {change:?}"
        );
    }
}

#[test]
fn failed_or_cancelled_supplied_candidate_does_not_publish_tentative_words() {
    let ir = original();
    let (component, proposal) = candidate(&ir);
    let mut wrong = proposal.clone();
    wrong
        .cells
        .values_mut()
        .find(|r| r.step != U256::ZERO)
        .unwrap()
        .step = U256::from(1);
    let mut verifier = Source::new(&ir, |_| true).unwrap();
    let incoming = entries(&mut verifier, &component);
    let before = verifier.admitted.clone();
    assert!(
        verifier
            .verify_candidate(&component, &wrong, &incoming)
            .is_none()
    );
    assert_eq!(
        verifier.admitted, before,
        "failed induction leaked a tentative header fact"
    );
    assert!(
        verifier
            .verify_candidate(&component, &proposal, &incoming)
            .is_some(),
        "failed candidate poisoned later valid verification"
    );

    let mut work = 0_usize;
    {
        let mut measured = Source::new(&ir, |cost| {
            work = work.saturating_add(cost);
            true
        })
        .unwrap();
        let incoming = entries(&mut measured, &component);
        assert!(
            measured
                .verify_candidate(&component, &proposal, &incoming)
                .is_some()
        );
    }
    for budget in [0, work / 2, work - 1] {
        let mut remaining = budget;
        let mut limited = match Source::new(&ir, |cost| {
            if cost > remaining {
                false
            } else {
                remaining -= cost;
                true
            }
        }) {
            Some(source) => source,
            None => continue,
        };
        let incoming = entries(&mut limited, &component);
        let before = limited.admitted.clone();
        assert!(
            limited
                .verify_candidate(&component, &proposal, &incoming)
                .is_none()
        );
        assert_eq!(
            limited.admitted, before,
            "a source budget refusal leaked tentative words"
        );
    }
}

#[test]
fn negative_affine_constant_in_a_feasible_bound_cannot_kill_the_source_edge() {
    let ir = original();
    let mut source = Source::new(&ir, |_| true).unwrap();
    let n = Atom::Source(EntityId(u64::MAX - 10));
    let condition = Atom::Source(EntityId(u64::MAX - 11));
    let expression = Expr::atom(n).sub(&Expr::constant(U256::from(1)));
    let mut state = State::default();
    state.atoms.insert(
        n,
        Range {
            low: U256::from(2),
            high: U256::from(10),
        },
    );
    state.predicates.insert(
        condition,
        Predicate {
            opcode: 0x11,
            inputs: vec![expression.clone(), Expr::constant(U256::from(9))],
        },
    );
    assert!(
        source.assume(&mut state, Expr::atom(condition), false, false),
        "n in [2,10] always satisfies n-1 <=9; modular -1 must stay signed while inverting the bound"
    );
    assert_eq!(
        state.range(&expression),
        Range {
            low: U256::from(1),
            high: U256::from(9)
        }
    );
    assert_eq!(
        state.atoms[&n],
        Range {
            low: U256::from(2),
            high: U256::from(10)
        }
    );
}

fn concrete(expression: &Expr, values: &BTreeMap<Atom, U256>) -> U256 {
    expression
        .terms
        .iter()
        .fold(expression.constant, |value, (atom, coefficient)| {
            value.wrapping_add(coefficient.wrapping_mul(values[atom]))
        })
}

#[test]
fn wrapped_relative_coefficients_cannot_hide_a_real_31_byte_overlap() {
    let ir = original();
    let source = Source::new(&ir, |_| true).unwrap();
    let x = Atom::Source(EntityId(u64::MAX - 20));
    let z = Atom::Source(EntityId(u64::MAX - 21));
    let half = U256::MAX >> 1;
    let values = BTreeMap::from([(x, U256::from(1)), (z, U256::MAX - U256::from(2))]);
    let mut state = State::default();
    for (&atom, &value) in &values {
        state.atoms.insert(atom, Range::exact(value));
    }
    let wide = Expr::atom(x).scale(half);
    let shift = Expr::constant(half - U256::from(128));
    let cell = wide.sub(&shift);
    let write = Expr::atom(z).sub(&wide).sub(&shift);
    assert_eq!(concrete(&cell, &values), U256::from(128));
    assert_eq!(concrete(&write, &values), U256::from(127));
    assert_eq!(state.ordinary(&cell), Some(Range::exact(U256::from(128))));
    assert_eq!(state.ordinary(&write), Some(Range::exact(U256::from(127))));
    assert!(
        !source.disjoint(&state, &write, &Expr::constant(U256::from(32)), &cell, 32),
        "coefficient cancellation modulo 2^256 hid 31 actually overwritten bytes"
    );
    let zero = Expr::constant(half).sub(&wide);
    assert_eq!(concrete(&zero, &values), U256::ZERO);
    assert!(!source.disjoint(
        &state,
        &zero,
        &Expr::constant(half + U256::from(1)),
        &wide,
        32
    ));
}

#[test]
fn singleton_affine_order_and_bound_claims_agree_with_independent_modular_evaluation() {
    let ir = original();
    let mut source = Source::new(&ir, |_| true).unwrap();
    let x = Atom::Source(EntityId(u64::MAX - 30));
    let z = Atom::Source(EntityId(u64::MAX - 31));
    let half = U256::MAX >> 1;
    let values = BTreeMap::from([(x, U256::from(1)), (z, U256::MAX - U256::from(2))]);
    let mut state = State::default();
    for (&atom, &value) in &values {
        state.atoms.insert(atom, Range::exact(value));
    }
    let mut expressions = Vec::new();
    for atom in [x, z] {
        for coefficient in [
            U256::ZERO,
            U256::from(1),
            U256::MAX,
            half,
            half + U256::from(1),
        ] {
            for constant in [
                U256::ZERO,
                U256::from(1),
                U256::from(31),
                U256::from(32),
                U256::from(127),
                U256::from(128),
                half,
                U256::MAX - U256::from(31),
            ] {
                expressions.push(
                    Expr::atom(atom)
                        .scale(coefficient)
                        .add(&Expr::constant(constant)),
                );
            }
        }
    }
    let wide = Expr::atom(x).scale(half);
    let shift = Expr::constant(half - U256::from(128));
    expressions.push(wide.sub(&shift));
    expressions.push(Expr::atom(z).sub(&wide).sub(&shift));
    for expression in &expressions {
        let actual = concrete(expression, &values);
        if let Some(range) = state.ordinary(expression) {
            assert!(
                range.low <= actual && actual <= range.high,
                "ordinary affine range excluded a feasible source assignment: {expression:?}"
            );
        }
        let containing = Range {
            low: actual.checked_sub(U256::from(1)).unwrap_or(U256::ZERO),
            high: actual.checked_add(U256::from(1)).unwrap_or(U256::MAX),
        };
        let mut narrowed = state.clone();
        assert!(
            source.restrict(&mut narrowed, expression, containing, false),
            "a true containing bound erased a feasible singleton assignment: {expression:?}"
        );
        for (&atom, &value) in &values {
            let range = narrowed.atoms.get(&atom).copied().unwrap_or(Range::FULL);
            assert!(range.low <= value && value <= range.high);
        }
        for other in &expressions {
            let target = concrete(other, &values);
            let (Some(end), Some(target_end)) = (
                actual.checked_add(U256::from(32)),
                target.checked_add(U256::from(32)),
            ) else {
                continue;
            };
            if source.disjoint(
                &state,
                expression,
                &Expr::constant(U256::from(32)),
                other,
                32,
            ) {
                assert!(
                    end <= target || target_end <= actual,
                    "admitted overlapping actual ranges [{actual},{end}) / [{target},{target_end}); write={expression:?}; cell={other:?}"
                );
            }
        }
    }
}

fn chained_candidate(ir: &Ir) -> (Vec<EntityId>, Candidate) {
    let mut source = Source::new(ir, |_| true).unwrap();
    let mut flows = BTreeMap::new();
    let mut first = true;
    for group in components(ir) {
        if group.len() > 1 {
            if first {
                source.loop_region(&group, &mut flows);
                assert_eq!(
                    source.certificates.len(),
                    1,
                    "the actual first loop must already be independently certified"
                );
                first = false;
            } else {
                let shape = source.shape(&group).unwrap();
                let initial = source.entry_state(&shape, &flows).unwrap();
                let proposal = source.propose(&shape, &initial).unwrap();
                return (group, proposal);
            }
        } else {
            let block = source.blocks[&group[0]];
            let flow = if Some(block.id) == ir.entry {
                Flow::Known(State {
                    memory: Some(block.memory_parameter),
                    effect: Some(block.effect_parameter),
                    ..State::default()
                })
            } else {
                source.join(block, &flows)
            };
            source.process(block, flow, None, false, &mut flows, false);
        }
    }
    panic!("independent fixture must have two sequential source loops")
}

fn accepts_chained(ir: &Ir, component: &[EntityId], proposal: &Candidate) -> bool {
    // Fresh verifier receives no proposer Flow facts. Its only loop premises
    // are the supplied flat certificates, checked against the current source.
    Source::new(ir, |_| true)
        .unwrap()
        .verify_candidate(component, proposal, &BTreeMap::new())
        .is_some()
}

#[derive(Clone, Copy, Debug)]
enum DependencyChange {
    Missing,
    Header,
    Step,
    Initial,
    Range,
    Bound,
    Key,
}

#[test]
fn predecessor_loop_certificates_are_replayed_without_trusting_a_stale_prefix() {
    let input = byte_input(true);
    for n in [0_u64, 1, 2, 7] {
        let mut host = crate::MemoryHost::new(&input, U256::from(n).to_be_bytes::<32>().to_vec());
        for _ in 0..2 * n {
            host.responses.push_back(crate::ExternalResponse {
                result: U256::from(1),
                return_data: vec![0xee; 96],
                ..crate::ExternalResponse::default()
            });
        }
        let raw =
            crate::execute_bytecode(&input, &mut host, &crate::ExecutionLimits::default()).unwrap();
        assert!(matches!(
            raw.outcome,
            crate::ExecutionOutcome::Exit {
                kind: crate::ExitKind::Stop,
                ..
            }
        ));
        assert_eq!(
            U256::from_be_slice(&host.memory[64..96]),
            U256::from(512 + 69 * n)
        );
        assert_eq!(
            raw.trace.iter().filter(|op| op.opcode == 0xfa).count(),
            usize::try_from(2 * n).unwrap()
        );
        assert!(host.responses.is_empty());
    }
    let ir = witness(true);
    let (component, original) = chained_candidate(&ir);
    assert_eq!(original.dependencies.len(), 1);
    assert!(accepts_chained(&ir, &component, &original));
    for change in [
        DependencyChange::Missing,
        DependencyChange::Header,
        DependencyChange::Step,
        DependencyChange::Initial,
        DependencyChange::Range,
        DependencyChange::Bound,
        DependencyChange::Key,
    ] {
        let mut wrong = original.clone();
        match change {
            DependencyChange::Missing => wrong.dependencies.clear(),
            DependencyChange::Key => {
                let (_, proof) = wrong.dependencies.pop_first().unwrap();
                wrong.dependencies.insert(EntityId(u64::MAX), proof);
            }
            _ => {
                let proof = wrong.dependencies.values_mut().next().unwrap();
                match change {
                    DependencyChange::Header => proof.header = original.header,
                    DependencyChange::Step => {
                        proof
                            .cells
                            .values_mut()
                            .find(|r| r.step != U256::ZERO)
                            .unwrap()
                            .step += U256::from(1)
                    }
                    DependencyChange::Initial => {
                        proof.cells.values_mut().next().unwrap().initial =
                            Expr::constant(U256::ZERO)
                    }
                    DependencyChange::Range => {
                        proof.scalars.values_mut().next().unwrap().range = Range::exact(U256::MAX)
                    }
                    DependencyChange::Bound => proof.bound = U256::ZERO,
                    _ => unreachable!(),
                }
            }
        }
        assert!(
            !accepts_chained(&ir, &component, &wrong),
            "accepted forged predecessor {change:?}"
        );
    }
    let prefix = original.dependencies.values().next().unwrap().clone();
    let prefix_component = components(&ir)
        .into_iter()
        .find(|group| group.contains(&prefix.header))
        .unwrap();
    let prefix_candidate = Candidate {
        proof: prefix,
        dependencies: BTreeMap::new(),
    };
    for change in [
        SourceChange::StepArgument,
        SourceChange::CounterNoProgress,
        SourceChange::HeaderInitialization,
        SourceChange::ExternalInitializer,
        SourceChange::EntryBound,
        SourceChange::CallOutputAlias,
        SourceChange::UnresolvedBackedge,
    ] {
        let mut wrong = ir.clone();
        corrupt(&mut wrong, &prefix_component, &prefix_candidate, change);
        assert!(
            !accepts_chained(&wrong, &component, &original),
            "target trusted an obsolete prefix certificate after {change:?}"
        );
    }
    let mut wrong = original.clone();
    wrong
        .dependencies
        .values_mut()
        .next()
        .unwrap()
        .cells
        .values_mut()
        .find(|r| r.step != U256::ZERO)
        .unwrap()
        .step += U256::from(1);
    let mut verifier = Source::new(&ir, |_| true).unwrap();
    let before = verifier.admitted.clone();
    assert!(
        verifier
            .verify_candidate(&component, &wrong, &BTreeMap::new())
            .is_none()
    );
    assert_eq!(
        verifier.admitted, before,
        "failed predecessor replay leaked target words"
    );
    assert!(
        verifier
            .verify_candidate(&component, &original, &BTreeMap::new())
            .is_some(),
        "an invalid predecessor poisoned a later valid candidate"
    );
}
