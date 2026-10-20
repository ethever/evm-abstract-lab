//! The interpreter owns transfer and discovers point tasks; no external worklist.
use super::language::{
    At, BOTH, Error, Instruction, Interval, MAX, Outcome, Outcomes, Parities, Pc, Program, State,
    States, Stop, Task, residue,
};
use analysis_kernel::{
    Accepts, Analyzer, Availability, Context, Either, Here, Kernel, KernelError, LocalClosure,
    Meter, Reachable, Read, Scoped, There,
};
use std::collections::BTreeSet;

pub struct Interpreter {
    pub program: Program,
    pub import_parities: bool,
    pub reverse: bool,
    pub admitted: BTreeSet<Scoped<Pc>>,
    pub trace: Vec<(Scoped<Pc>, Availability<State>)>,
}
impl Interpreter {
    pub fn discover<M: Meter<Stop = Stop>>(
        &mut self,
        at: At,
        kernel: &mut Kernel<Task>,
        meter: &mut M,
    ) -> Result<(), KernelError<Stop>> {
        // Pay for task construction and the client-owned deduplication set.
        meter.checkpoint().map_err(KernelError::Stopped)?;
        meter
            .reserve(self.admitted.len().saturating_mul(4).saturating_add(16))
            .map_err(KernelError::Stopped)?;
        if self.admitted.contains(&at.key()) {
            return Ok(());
        }
        let flow = Either::Right(Either::Left(at));
        let parity = Either::Right(Either::Right(at));
        if self.import_parities && self.reverse {
            kernel.register(at.scope, parity, meter)?;
            kernel.register(at.scope, flow, meter)?;
        } else {
            kernel.register(at.scope, flow, meter)?;
            if self.import_parities {
                kernel.register(at.scope, parity, meter)?;
            }
        }
        self.admitted.insert(at.key());
        Ok(())
    }
}

impl<'r, 'k, M, S> Analyzer<Context<'r, 'k, Task, M, S>> for Interpreter
where
    M: Meter<Stop = Stop>,
    Context<'r, 'k, Task, M, S>: Read<States, Error = KernelError<Stop>>
        + Read<Parities, There<Here>, Error = KernelError<Stop>>
        + Accepts<States, Error = Error>
        + Accepts<Outcomes, There<There<Here>>, Error = Error>,
{
    type Task = At;
    type Error = Error;
    fn evaluate(
        &mut self,
        at: &At,
        cx: &mut Context<'r, 'k, Task, M, S>,
    ) -> Result<LocalClosure, Error> {
        // Fixed-size domains: at most eight words and two successor writes.
        cx.run()
            .reserve(self.trace.len().saturating_mul(8).saturating_add(256))?;
        let input = <_ as Read<States>>::query(cx, at.key())?;
        self.trace.push((at.key(), input.state.clone()));
        let mask = if self.import_parities {
            let answer = <_ as Read<Parities, There<Here>>>::query(cx, at.key())?;
            let Some(mask) = answer.value() else {
                return Ok(LocalClosure::Stable);
            };
            *mask
        } else {
            BOTH
        };
        let Availability::Value(Reachable::Value(range)) = input.state else {
            return Ok(LocalClosure::Stable);
        };
        // Temporary reduced product. Both ascending inputs may grow; nothing
        // narrowed here is retained as a descending guarantee on future inputs.
        let words = (range.lo..=range.hi).filter(|word| mask & residue(*word) != 0);
        let mut successors = Vec::with_capacity(2);
        match self.program.at(at.pc) {
            Instruction::Equal { value, yes, no } => {
                successors.push((yes, Interval::cover(words.clone().filter(|w| *w == value))));
                successors.push((no, Interval::cover(words.filter(|w| *w != value))));
            }
            Instruction::Less { limit, yes, no } => {
                successors.push((yes, Interval::cover(words.clone().filter(|w| *w < limit))));
                successors.push((no, Interval::cover(words.filter(|w| *w >= limit))));
            }
            Instruction::Add { amount, next } => {
                successors.push((
                    next,
                    Interval::cover(words.map(|w| w.saturating_add(amount).min(MAX))),
                ));
            }
            Instruction::Return | Instruction::Fault => {
                let outcome = match self.program.at(at.pc) {
                    Instruction::Return => Outcome::Returned,
                    _ => Outcome::Faulted,
                };
                <_ as Accepts<Outcomes, There<There<Here>>>>::accept(
                    cx,
                    Scoped::new(at.scope, outcome),
                    Interval::cover(words),
                )?;
            }
        }
        for (pc, state) in successors {
            if state == Reachable::Bottom {
                continue;
            }
            <_ as Accepts<States>>::accept(cx, Scoped::new(at.scope, pc), state)?;
            let (kernel, meter) = cx.run().parts();
            self.discover(
                At {
                    scope: at.scope,
                    pc,
                },
                kernel,
                meter,
            )?;
        }
        Ok(LocalClosure::Stable)
    }
}
