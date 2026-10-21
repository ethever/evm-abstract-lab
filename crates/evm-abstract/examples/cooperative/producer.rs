//! This proof producer knows expressions and ZeroGuards, never writes StackFacts.
use super::contracts::{Failure, ProofRoute, ZeroGuards};
use evm_abstract::{
    U256,
    analysis::cooperation::{
        InputRoute, Inputs, RefinementRoute, Refinements, StackFacts, Subject,
    },
    domain::{
        WordElement,
        symbolic::{ExprId, ExprKind},
    },
    kernel::{Accepts, Charges, LocalClosure, Read, Reduction, Scoped},
};
use revm_bytecode::opcode;
use std::{collections::BTreeSet, marker::PhantomData};

pub struct Producer<N: WordElement> {
    enabled: bool,
    pub feedback_observed: usize,
    numeric: PhantomData<N>,
}
impl<N: WordElement> Producer<N> {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            feedback_observed: 0,
            numeric: PhantomData,
        }
    }
}

fn always_false(expression: &ExprId) -> bool {
    let ExprKind::Operation { opcode: op, args } = expression.kind() else {
        return false;
    };
    *op == opcode::EQ
        && args.len() == 2
        && (0..2).any(|zero| {
            if !matches!(args[zero].kind(), ExprKind::Constant(c) if *c == U256::ZERO) {
                return false;
            }
            let ExprKind::Operation {
                opcode: inner,
                args,
            } = args[1 - zero].kind()
            else {
                return false;
            };
            // Bitwise OR preserves every set bit of either operand, including bit 255.
            // This law covers every U256 input, with no hidden calldata assumption.
            *inner == opcode::OR
                && args.len() == 2
                && args
                    .iter()
                    .any(|a| matches!(a.kind(), ExprKind::Constant(c) if *c != U256::ZERO))
        })
}

impl<N: WordElement, Cx> Reduction<Cx> for Producer<N>
where
    Cx: Read<Inputs<N>, InputRoute>
        + Read<Refinements, RefinementRoute, Error = <Cx as Read<Inputs<N>, InputRoute>>::Error>
        + Accepts<ZeroGuards, ProofRoute>
        + Charges<Error = <Cx as Read<Inputs<N>, InputRoute>>::Error>,
{
    type Task = Subject;
    type Error = Failure<
        <Cx as Read<Inputs<N>, InputRoute>>::Error,
        <Cx as Accepts<ZeroGuards, ProofRoute>>::Error,
    >;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.charge(64).map_err(Failure::Infrastructure)?;
        let input = <Cx as Read<Inputs<N>, InputRoute>>::query(cx, Scoped::new(subject.scope, ()))
            .map_err(Failure::Infrastructure)?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        if input.subject() != *subject {
            return Err(Failure::WrongSubject);
        }
        // A tracked feedback read also demonstrates the reverse wake-up edge.
        let feedback =
            <Cx as Read<Refinements, RefinementRoute>>::query(cx, Scoped::new(subject.scope, ()))
                .map_err(Failure::Infrastructure)?;
        if matches!(feedback.value(), Some(StackFacts::Reachable(f)) if !f.is_empty()) {
            self.feedback_observed += 1;
        }
        let stack = &input.payload().active().stack;
        cx.charge(stack.len().saturating_add(1))
            .map_err(Failure::Infrastructure)?;
        let mut proofs = BTreeSet::new();
        if self.enabled {
            for (index, value) in stack.iter().enumerate() {
                cx.charge(value.work_size().saturating_mul(16).saturating_add(16))
                    .map_err(Failure::Infrastructure)?;
                if value.expression().is_some_and(always_false) {
                    proofs.insert(index);
                }
            }
        }
        <Cx as Accepts<ZeroGuards, ProofRoute>>::accept(cx, Scoped::new(subject.scope, ()), proofs)
            .map_err(Failure::Publication)?;
        Ok(LocalClosure::Stable)
    }
}
