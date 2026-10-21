//! An external program analyzer, written solely against public typed capabilities.
use super::super::carrier::ExcludingWord;
use alloy_primitives::U256;
use analysis_kernel::{Accepts, Change, Charges, LocalClosure, Read, Reduction, Scoped};
use evm_abstract::{
    analysis::cooperation::{
        InputRoute, Inputs, RefinementRoute, Refinements, StackFacts, Subject,
    },
    domain::{
        WordElement, WordObservation,
        symbolic::{ExprId, ExprKind},
    },
};
use revm_bytecode::opcode;
use std::fmt;

#[derive(Debug)]
pub enum Failure<K, P> {
    Infrastructure(K),
    Publication(P),
    WrongSubject,
}
impl<K: fmt::Display, P: fmt::Display> fmt::Display for Failure<K, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Infrastructure(error) => error.fmt(f),
            Self::Publication(error) => error.fmt(f),
            Self::WrongSubject => f.write_str("immutable input belongs to another Subject"),
        }
    }
}
#[derive(Default)]
pub struct Rule {
    pub enabled: bool,
    pub subjects: Vec<Subject>,
    pub feedback: Vec<(Subject, Change)>,
    pub exhaust_after_feedback: bool,
    pub has_strengthened: bool,
}
impl Rule {
    fn compares_excluded_word(expression: &ExprId, value: &ExprId) -> bool {
        let ExprKind::Operation { opcode: op, args } = expression.kind() else {
            return false;
        };
        *op == opcode::EQ
            && args.len() == 2
            && (0..2).any(|constant| {
                matches!(args[constant].kind(), ExprKind::Constant(word) if *word == U256::from(42))
                    && args[1 - constant] == *value
            })
    }
}
impl<Cx> Reduction<Cx> for Rule
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute>
        + Read<
            Refinements,
            RefinementRoute,
            Error = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::Error,
        > + Accepts<Refinements, RefinementRoute>
        + Charges<Error = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::Error>,
{
    type Task = Subject;
    type Error = Failure<
        <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::Error,
        <Cx as Accepts<Refinements, RefinementRoute>>::Error,
    >;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.charge(64).map_err(Failure::Infrastructure)?;
        if self.exhaust_after_feedback && self.has_strengthened {
            cx.charge(10_000_000_000).map_err(Failure::Infrastructure)?;
        }
        let input = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )
        .map_err(Failure::Infrastructure)?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        if input.subject() != *subject {
            return Err(Failure::WrongSubject);
        }
        // Reading feedback records a self-dependency. Changed -> reevaluation ->
        // equal publication is the kernel's responsibility, not a manual loop.
        <Cx as Read<Refinements, RefinementRoute>>::query(cx, Scoped::new(subject.scope, ()))
            .map_err(Failure::Infrastructure)?;
        self.subjects.push(*subject);
        let mut facts = StackFacts::none();
        if self.enabled {
            let stack = &input.payload().active().stack;
            for (index, pair) in stack.windows(2).enumerate() {
                // The custom work quotes are constant-time bounds; reserve before
                // reading semantic constraints, cloning or comparing expressions.
                cx.charge(
                    pair[0]
                        .work_size()
                        .saturating_add(pair[1].work_size())
                        .saturating_mul(4),
                )
                .map_err(Failure::Infrastructure)?;
                if !pair[0].numeric().contains(U256::from(42))
                    && let (Some(value), Some(guard)) =
                        (pair[0].symbolic_expression(), pair[1].expression())
                    && Self::compares_excluded_word(guard, &value)
                {
                    // EQ(v,42)==0 follows from THIS immutable input's actual v,
                    // not from a label, PC heuristic, previous generation or tag.
                    facts = StackFacts::at(index + 1, WordObservation::constant(U256::ZERO));
                    break;
                }
            }
        }
        let change = <Cx as Accepts<Refinements, RefinementRoute>>::accept(
            cx,
            Scoped::new(subject.scope, ()),
            facts,
        )
        .map_err(Failure::Publication)?;
        self.has_strengthened |= change == Change::Changed;
        self.feedback.push((*subject, change));
        Ok(LocalClosure::Stable)
    }
}
