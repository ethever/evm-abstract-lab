//! Only the executed parent adopts X; its same-edge successor adopts the
//! captured execution receipt through Inputs, without knowing X's capability.
use super::{
    deferred::{Factory, GuardBound, Source},
    *,
};
use evm_abstract::analysis::{OutcomeKind, cooperation::Input};

struct Parent {
    pc: usize,
    widen: bool,
    snapshots: Vec<(
        Subject,
        evm_abstract::analysis::EvmMachinePayload<ExcludingWord>,
    )>,
    children: Vec<(Subject, ExcludingWord)>,
}
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
impl<Cx> Reduction<Cx> for Parent
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute, Error = Infrastructure>
        + Read<GuardBound, SharedRoute<Here>, Error = Infrastructure>
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
        cx.charge(input.work_size().saturating_mul(4))?;
        if subject.point.namespace == 2 && pc(input) == Some(self.pc + 5) {
            self.children.push((
                *subject,
                input.payload().active().stack[0].numeric().clone(),
            ));
        }
        if subject.point.namespace != 1 || pc(input) != Some(self.pc) {
            return Ok(LocalClosure::Stable);
        }
        self.snapshots.push((*subject, input.payload().clone()));
        let bound = <Cx as Read<GuardBound, SharedRoute<Here>>>::query(
            cx,
            Scoped::new(input.universe(), ()),
        )?;
        let Some(widen) = bound.value() else {
            return Ok(LocalClosure::Stable);
        };
        if !self.widen || !*widen {
            // Every path computes (v & 1) | 1 == 1. The deliberate
            // native projection [1,2] loses that complete invariant. X controls
            // a legal precision loss after a closed result, never a proof from
            // an incomplete producer/edge inventory.
            <Cx as Accepts<Refinements, RefinementRoute>>::accept(
                cx,
                Scoped::new(subject.scope, ()),
                StackFacts::at(0, WordObservation::constant(U256::ONE)),
            )
            .map_err(Failure::Feedback)?;
        }
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn parent_only_assumption_expands_the_same_child_snapshot_and_actual_return() {
    for use_summaries in [false, true] {
        for widen in [false, true] {
            let fixture = super::super::bridge::fixture::guarded(true);
            let mut code = fixture.code;
            let preparation = code.len();
            let a = preparation + 11;
            let b = a + 5;
            // Preserve the recursive fixture's existing targets. Its ordinary
            // terminal computes a complete invariant before jumping through A.
            code[fixture.impossible - 9..fixture.impossible].copy_from_slice(&[
                opcode::PUSH2,
                (preparation >> 8) as u8,
                preparation as u8,
                opcode::JUMP,
                opcode::STOP,
                opcode::STOP,
                opcode::STOP,
                opcode::STOP,
                opcode::STOP,
            ]);
            code.extend([
                opcode::JUMPDEST,
                opcode::PUSH1,
                1,
                opcode::AND,
                opcode::PUSH1,
                1,
                opcode::OR,
                opcode::PUSH2,
                (a >> 8) as u8,
                a as u8,
                opcode::JUMP,
                opcode::JUMPDEST,
                opcode::PUSH2,
                (b >> 8) as u8,
                b as u8,
                opcode::JUMP,
                opcode::JUMPDEST,
                opcode::PUSH0,
                opcode::MSTORE,
                opcode::PUSH1,
                32,
                opcode::PUSH0,
                opcode::RETURN,
            ]);
            let calls = Calls::default();
            let mut policy = Policy::new(&calls);
            policy.coarsen_one = true;
            let algebra = evm_abstract::domain::ValueAlgebra::new(policy);
            let mut extension = Extension::with_tasks(
                Pair::new(
                    Source::default(),
                    Registered::new(Parent {
                        pc: a,
                        widen,
                        snapshots: Vec::new(),
                        children: Vec::new(),
                    }),
                ),
                Order(true),
            )
            .with_shared_stores(Factory);
            let mut config = ExecutionConfig {
                max_work: 5_000_000_000,
                root_terminal_coverage: false,
                use_summaries,
                ..ExecutionConfig::default()
            };
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
            assert!(extension.analyzer.left.grown > 0);
            let parent = extension.analyzer.right.rule();
            assert!(
                parent
                    .snapshots
                    .iter()
                    .any(|(_, payload)| payload.active().stack[0]
                        .numeric()
                        .contains(U256::from(2))),
                "native parent must cover both values"
            );
            assert!(
                parent
                    .snapshots
                    .iter()
                    .enumerate()
                    .any(
                        |(i, (old, raw))| parent.snapshots[i + 1..].iter().any(|(new, next)| old
                            .point
                            == new.point
                            && old.scope != new.scope
                            && raw == next)
                    ),
                "same native snapshot must be re-executed under a new receipt"
            );
            let narrow = parent
                .children
                .iter()
                .find(|(_, value)| value.singleton() == Some(U256::ONE))
                .expect("first derived snapshot is {1}");
            assert_eq!(
                parent
                    .children
                    .iter()
                    .any(|(subject, value)| subject.point == narrow.0.point
                        && subject.scope != narrow.0.scope
                        && value.contains(U256::from(2))),
                widen,
                "same derived Point must receive the fresh {{1,2}} payload"
            );
            assert_eq!(
                result
                    .procedure_summaries()
                    .iter()
                    .flat_map(|record| &record.procedures)
                    .flat_map(|procedure| &procedure.outputs)
                    .filter(|output| output.kind == OutcomeKind::Return)
                    .any(|output| output
                        .data
                        .read_word_with(&fixtures::word(0), algebra.borrowed())
                        .contains(U256::from(2))),
                widen
            );
            assert!(
                parent.snapshots.len() < 50,
                "unchanged receipts must settle"
            );
        }
    }
}
