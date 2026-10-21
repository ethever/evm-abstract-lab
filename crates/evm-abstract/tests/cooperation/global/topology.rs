//! A late contributor is numerically covered but changes the declared universe.
use super::*;

struct Arrivals {
    source: usize,
    late: usize,
    target: usize,
    // Diagnostic copies only: rule evaluation never consults this trace.
    snapshots: Vec<(
        Subject,
        evm_abstract::analysis::EvmMachinePayload<ExcludingWord>,
        usize,
        bool,
    )>,
}

impl<Cx> Reduction<Cx> for Arrivals
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<Generations, GenerationRoute, Error = Infrastructure>
        + ReadGeneration<Error = Infrastructure>
        + Read<Topology, TopologyRoute, Error = Infrastructure>
        + Accepts<Refinements, RefinementRoute, Error = PublishError<FactError, MachineError>>
        + Charges<Error = Infrastructure>,
{
    type Task = Subject;
    type Error = Failure;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Failure> {
        let answer = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )?;
        let Some(input) = answer.value() else {
            return Ok(LocalClosure::Stable);
        };
        if fixtures::input_pc(input) != Some(self.target) {
            return Ok(LocalClosure::Stable);
        }
        cx.charge(input.work_size().saturating_mul(4))?;
        let neighbors = <Cx as Read<Topology, TopologyRoute>>::query(
            cx,
            Scoped::new(
                input.universe(),
                Neighbors {
                    point: subject.point,
                    direction: Direction::Predecessors,
                },
            ),
        )?;
        let Some(edges) = neighbors.value() else {
            return Ok(LocalClosure::Stable);
        };
        let mut zero = !edges.is_empty();
        for edge in edges {
            let source_answer = current_input::<ExcludingWord, _>(cx, input.universe(), edge.from)?;
            let Some(source) = source_answer.value() else {
                return Ok(LocalClosure::Stable);
            };
            assert_eq!(source_answer.scope, source.subject().scope);
            cx.charge(source.work_size().saturating_mul(4))?;
            match fixtures::input_pc(source) {
                Some(pc) if pc == self.source => {
                    // The source arm is reached only when the shared input
                    // bit is zero (ISZERO reverses the physical arm ordering).
                    // JUMPs preserve that very same native bit expression.
                    zero &= source
                        .payload()
                        .active()
                        .stack
                        .first()
                        .is_some_and(|word| word.numeric().contains(U256::ZERO));
                }
                Some(pc) if pc == self.late => zero = false,
                _ => return Ok(LocalClosure::Stable),
            }
        }
        // Coverage is this frozen Subject's actual contributor universe. The
        // independent late arm is explored without consulting this must fact;
        // discovering it invalidates the guarantee before that arm is consumed.
        let bound = if zero {
            WordObservation::constant(U256::ZERO)
        } else {
            WordObservation::unsigned_range(U256::ZERO, U256::ONE).unwrap()
        };
        <Cx as Accepts<Refinements, RefinementRoute>>::accept(
            cx,
            Scoped::new(subject.scope, ()),
            StackFacts::at(0, bound),
        )
        .map_err(Failure::Feedback)?;
        self.snapshots
            .push((*subject, input.payload().clone(), edges.len(), zero));
        Ok(LocalClosure::Stable)
    }
}
fn fixture(backedge: bool, reverse: bool) -> (Vec<u8>, usize, usize, usize, usize) {
    // Both arms carry the SAME byte-root/expression/native [0,1] payload. Only
    // their path meaning differs. This isolates genuinely covered admission
    // from expression/store-root representation growth at the target.
    let mut code = vec![
        opcode::PUSH0,
        opcode::CALLDATALOAD,
        opcode::PUSH1,
        1,
        opcode::AND,
        opcode::DUP1,
    ];
    if reverse {
        code.push(opcode::ISZERO);
    }
    code.push(opcode::PUSH2);
    let true_target = code.len();
    code.extend([0, 0, opcode::JUMPI, opcode::PUSH2]);
    let fall_target = code.len();
    code.extend([0, 0, opcode::JUMP]);
    let arm = code.len();
    code.extend([opcode::JUMPDEST, opcode::PUSH2, 0, 0, opcode::JUMP]);
    code[true_target..true_target + 2].copy_from_slice(&(arm as u16).to_be_bytes());
    let a = code.len();
    code.extend([opcode::JUMPDEST, opcode::PUSH2, 0, 0, opcode::JUMP]);
    let d0 = code.len();
    code.extend([opcode::JUMPDEST, opcode::PUSH2, 0, 0, opcode::JUMP]);
    let d1 = code.len();
    code[d0 + 2..d0 + 4].copy_from_slice(&(d1 as u16).to_be_bytes());
    code.extend([opcode::JUMPDEST, opcode::PUSH2, 0, 0, opcode::JUMP]);
    let d = code.len();
    code[d1 + 2..d1 + 4].copy_from_slice(&(d as u16).to_be_bytes());
    code.extend([opcode::JUMPDEST, opcode::PUSH2, 0, 0, opcode::JUMP]);
    let b = code.len();
    code[a + 2..a + 4].copy_from_slice(&(b as u16).to_be_bytes());
    code[d + 2..d + 4].copy_from_slice(&(b as u16).to_be_bytes());
    let targets = if reverse { [d0, a] } else { [a, d0] };
    code[fall_target..fall_target + 2].copy_from_slice(&(targets[0] as u16).to_be_bytes());
    code[arm + 2..arm + 4].copy_from_slice(&(targets[1] as u16).to_be_bytes());
    code.extend([
        opcode::JUMPDEST,
        opcode::DUP1,
        opcode::PUSH2,
        0,
        0,
        opcode::JUMPI,
    ]);
    if backedge {
        code.extend([opcode::PUSH2, (d0 >> 8) as u8, d0 as u8, opcode::JUMP]);
    } else {
        code.push(opcode::STOP);
    }
    let restored = code.len();
    code[b + 3..b + 5].copy_from_slice(&(restored as u16).to_be_bytes());
    code.extend([
        opcode::JUMPDEST,
        opcode::PUSH1,
        9,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::RETURN,
    ]);
    (code, a, d, b, restored)
}
#[test]
fn covered_late_edge_and_backedge_reseed_old_must_facts_and_restore_a_feasible_branch() {
    for backedge in [false, true] {
        for reverse in [false, true] {
            let (code, source, late, target, restored) = fixture(backedge, reverse);
            let calls = Calls::default();
            let algebra = fixtures::feedback_algebra(&calls);
            let mut extension = Extension::new(Registered::new(Arrivals {
                source,
                late,
                target,
                snapshots: Vec::new(),
            }));
            let mut config = fixtures::config(5_000_000_000);
            config.analysis.context_depth = 0;
            config.analysis.relations.enabled = false;
            let mut entry = fixtures::entry();
            entry.environment.calldata = evm_abstract::world::EvmByteArray::unknown();
            let result = analyze_world_cooperating(
                fixtures::world(&code),
                entry,
                config,
                &algebra,
                &EnginePolicies::default(),
                &mut extension,
            )
            .unwrap();
            fixtures::complete(&result);
            let trace = &extension.analyzer.rule().snapshots;
            let first = trace
                .iter()
                .find(|(_, _, edges, zero)| *edges == 1 && *zero)
                .expect("first contributor establishes a valid local zero guarantee");
            let grown = trace
                .iter()
                .find(|(_, _, edges, zero)| *edges == 2 && !*zero)
                .expect("new topology must retire the old must fact");
            assert_ne!(first.0.scope, grown.0.scope);
            if !backedge {
                assert_eq!(
                    first.1, grown.1,
                    "covered late edge changes topology while the complete native input stays identical"
                );
            } else {
                assert_eq!(
                    first.1.active().stack[0].numeric(),
                    grown.1.active().stack[0].numeric(),
                    "the backedge preserves numeric coverage while guarded expression metadata can grow"
                );
            }
            assert!(
                result
                    .states()
                    .iter()
                    .any(|state| state.executed_pcs.contains(&restored)),
                "feasible late branch must be restored"
            );
            assert!(
                trace.len() < 50,
                "covered cycles must settle in finitely many generations"
            );
        }
    }
}
