//! Two independent proof components exchange inequalities through a real cycle.
use super::*;
use evm_abstract::analysis::cooperation::Input;
use std::collections::BTreeMap;

fn pc(input: &Input<ExcludingWord>) -> Option<usize> {
    let frame = input.payload().active();
    Some(
        frame
            .program
            .as_ref()?
            .blocks()
            .get(frame.key.basic_block_index)?
            .start_pc,
    )
}
struct Inequality {
    upper: bool,
    enabled: bool,
    a: usize,
    b: usize,
    c: usize,
    exact: BTreeSet<Subject>,
    guard: BTreeSet<Subject>,
    subjects: BTreeSet<Subject>,
}
impl<Cx> Reduction<Cx> for Inequality
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<Generations, GenerationRoute, Error = Infrastructure>
        + ReadGeneration<Error = Infrastructure>
        + Read<Topology, TopologyRoute, Error = Infrastructure>
        + Discover<Topology, TopologyRoute, Error = Infrastructure>
        + Read<Refinements, RefinementRoute, Error = Infrastructure>
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
        let own_pc = if self.upper { self.a } else { self.b };
        let own = pc(input);
        if own != Some(own_pc) && (self.upper || own != Some(self.c)) {
            return Ok(LocalClosure::Stable);
        }
        self.subjects.insert(*subject);
        if !self.upper && !self.enabled {
            return Ok(LocalClosure::Stable);
        }
        cx.charge(input.work_size().saturating_mul(8))?;
        if !self.upper && own == Some(self.c) {
            let edges = <Cx as Read<Topology, TopologyRoute>>::query(
                cx,
                Scoped::new(
                    input.universe(),
                    Neighbors {
                        point: subject.point,
                        direction: Direction::Predecessors,
                    },
                ),
            )?;
            if let Some(edges) = edges.value() {
                for edge in edges {
                    let source =
                        current_input::<ExcludingWord, _>(cx, input.universe(), edge.from)?;
                    let Some(source) = source.value() else {
                        continue;
                    };
                    if pc(source) != Some(self.b) {
                        continue;
                    }
                    let facts = <Cx as Read<Refinements, RefinementRoute>>::query(
                        cx,
                        Scoped::new(source.subject().scope, ()),
                    )?;
                    if let Some(StackFacts::Reachable(facts)) = facts.value()
                        && facts
                            .get(&1)
                            .is_some_and(|fact| fact.singleton() == Some(U256::ONE))
                    {
                        <Cx as Accepts<Refinements, RefinementRoute>>::accept(
                            cx,
                            Scoped::new(subject.scope, ()),
                            StackFacts::at(1, WordObservation::constant(U256::ONE)),
                        )
                        .map_err(Failure::Feedback)?;
                        self.guard.insert(*subject);
                    }
                }
            }
            return Ok(LocalClosure::Stable);
        }

        // Independent bitwise proofs about v=(storage[0]&1)|1: upper proves
        // v<=1; lower proves v>=1. Each is valid without assuming peer progress.
        let initial = if self.upper {
            WordObservation::unsigned_range(U256::ZERO, U256::ONE).unwrap()
        } else {
            WordObservation::unsigned_range(U256::ONE, U256::MAX).unwrap()
        };
        let mut facts = BTreeMap::from([(0, initial)]);
        let direction = if self.upper {
            Direction::Successors
        } else {
            Direction::Predecessors
        };
        let edges = <Cx as Discover<Topology, TopologyRoute>>::discover(
            cx,
            Scoped::new(
                input.universe(),
                Neighbors {
                    point: subject.point,
                    direction,
                },
            ),
        )?;
        if let Some(edges) = edges.value() {
            for edge in edges {
                let point = if self.upper { edge.to } else { edge.from };
                let peer = current_input::<ExcludingWord, _>(cx, input.universe(), point)?;
                let Some(peer) = peer.value() else {
                    continue;
                };
                if pc(peer) != Some(if self.upper { self.b } else { self.a }) {
                    continue;
                }
                let answer = <Cx as Read<Refinements, RefinementRoute>>::query(
                    cx,
                    Scoped::new(peer.subject().scope, ()),
                )?;
                let Some(StackFacts::Reachable(peer_facts)) = answer.value() else {
                    continue;
                };
                let Some(bound) = peer_facts.get(&0) else {
                    continue;
                };
                if self.upper && self.enabled && bound.interval().unsigned_bounds().0 >= U256::ONE {
                    facts.insert(0, WordObservation::constant(U256::ONE));
                    self.exact.insert(*subject);
                } else if !self.upper && bound.singleton() == Some(U256::ONE) {
                    // The return edge carries the unchanged v and its EQ(v,1).
                    // Only the peer's new exact bound enables this consequence.
                    facts.insert(1, WordObservation::constant(U256::ONE));
                    self.guard.insert(*subject);
                }
            }
        }
        <Cx as Accepts<Refinements, RefinementRoute>>::accept(
            cx,
            Scoped::new(subject.scope, ()),
            StackFacts::Reachable(facts),
        )
        .map_err(Failure::Feedback)?;
        Ok(LocalClosure::Stable)
    }
}
fn fixture() -> (Vec<u8>, usize, usize, usize, usize) {
    let mut code = vec![
        opcode::PUSH0,
        opcode::SLOAD,
        opcode::PUSH1,
        1,
        opcode::AND,
        opcode::PUSH1,
        1,
        opcode::OR,
        opcode::DUP1,
        opcode::PUSH1,
        1,
        opcode::EQ,
        opcode::PUSH2,
        0,
        0,
        opcode::JUMP,
    ];
    let a = code.len();
    code[13..15].copy_from_slice(&(a as u16).to_be_bytes());
    code.extend([opcode::JUMPDEST, opcode::PUSH2, 0, 0, opcode::JUMP]);
    let b = code.len();
    code[a + 2..a + 4].copy_from_slice(&(b as u16).to_be_bytes());
    code.extend([opcode::JUMPDEST, opcode::PUSH2, 0, 0, opcode::JUMP]);
    let c = code.len();
    code[b + 2..b + 4].copy_from_slice(&(c as u16).to_be_bytes());
    code.extend([
        opcode::JUMPDEST,
        opcode::PUSH2,
        0,
        0,
        opcode::JUMPI,
        opcode::INVALID,
    ]);
    let impossible = c + 5;
    let success = code.len();
    code[c + 2..c + 4].copy_from_slice(&(success as u16).to_be_bytes());
    code.extend([
        opcode::JUMPDEST,
        opcode::POP,
        opcode::PUSH1,
        1,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::RETURN,
    ]);
    (code, a, b, c, impossible)
}
#[test]
fn peer_feedback_strengthens_source_then_returns_a_new_branch_consequence_without_reset_loop() {
    for upper_first in [false, true] {
        for (forward, backward) in [(true, true), (false, true), (true, false)] {
            let (code, a, b, c, impossible) = fixture();
            let calls = Calls::default();
            let mut policy = Policy::new(&calls);
            policy.exclusion_queries = false;
            let algebra = evm_abstract::domain::ValueAlgebra::new(policy);
            let rule = |upper, enabled| Inequality {
                upper,
                enabled,
                a,
                b,
                c,
                exact: BTreeSet::new(),
                guard: BTreeSet::new(),
                subjects: BTreeSet::new(),
            };
            let mut extension = Extension::with_tasks(
                Pair::new(
                    Registered::new(rule(true, forward)),
                    Registered::new(rule(false, backward)),
                ),
                Order(upper_first),
            );
            let mut config = ExecutionConfig {
                max_work: 5_000_000_000,
                root_terminal_coverage: false,
                ..ExecutionConfig::default()
            };
            // Preserve the coarse native EQ carrier; neither ordinary relation
            // inference nor one local inequality supplies the peer's guarantee.
            config.analysis.relations.enabled = false;
            let result = analyze_world_cooperating(
                fixtures::world(&code),
                fixtures::entry(),
                config,
                &algebra,
                &EnginePolicies::default(),
                &mut extension,
            )
            .unwrap();
            assert_eq!(
                result.status(),
                Status::Converged,
                "{:?}",
                result.frontiers()
            );
            let complete = forward && backward;
            assert_eq!(
                !extension.analyzer.left.rule().exact.is_empty(),
                complete,
                "B's lower bound must materially strengthen A"
            );
            assert_eq!(
                !extension.analyzer.right.rule().guard.is_empty(),
                complete,
                "A's exact feedback must produce B's new guard fact"
            );
            assert_eq!(
                result
                    .states()
                    .iter()
                    .any(|state| state.executed_pcs.contains(&impossible)),
                !complete,
                "removing either semantic direction must lose real CFG precision"
            );
            assert!(
                result
                    .outcomes()
                    .iter()
                    .any(|outcome| outcome.kind == evm_abstract::analysis::OutcomeKind::Return)
            );
            assert!(extension.analyzer.left.rule().subjects.len() < 16);
            assert!(extension.analyzer.right.rule().subjects.len() < 32);
        }
    }
}
