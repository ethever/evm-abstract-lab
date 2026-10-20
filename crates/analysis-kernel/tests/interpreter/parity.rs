//! Independent forward residue analysis. It shares vocabulary, not interpreter
//! implementation or private state; its only inputs are declared capabilities.
use super::language::{
    At, Error, Instruction, Interval, MAX, Parities, Pc, Program, State, States, Stop, residue,
};
use analysis_kernel::{
    Accepts, Analyzer, Availability, Charges, Here, KernelError, LocalClosure, Reachable, Read,
    Scoped, There,
};

pub struct Parity {
    pub program: Program,
    pub import_intervals: bool,
    pub trace: Vec<(Scoped<Pc>, Availability<State>)>,
}
fn cover(words: impl Iterator<Item = u8>) -> u8 {
    words.fold(0, |mask, word| mask | residue(word))
}
impl<Cx> Analyzer<Cx> for Parity
where
    Cx: Read<States, Error = KernelError<Stop>>
        + Read<Parities, There<Here>, Error = KernelError<Stop>>
        + Accepts<Parities, There<Here>, Error = Error>
        + Charges<Error = KernelError<Stop>>,
{
    type Task = At;
    type Error = Error;
    fn evaluate(&mut self, at: &At, cx: &mut Cx) -> Result<LocalClosure, Error> {
        cx.charge(self.trace.len().saturating_mul(8).saturating_add(256))?;
        let input = <Cx as Read<States>>::query(cx, at.key())?;
        let residues = <Cx as Read<Parities, There<Here>>>::query(cx, at.key())?;
        self.trace.push((at.key(), input.state.clone()));
        let (Availability::Value(Reachable::Value(range)), Some(mask)) =
            (input.state, residues.value())
        else {
            return Ok(LocalClosure::Stable);
        };
        // The ablation supplies sound Top bounds, retaining reachability gating.
        let range = if self.import_intervals {
            range
        } else {
            Interval::TOP
        };
        let words = (0..=MAX).filter(|w| mask & residue(*w) != 0 && range.contains(*w));
        let mut successors = Vec::with_capacity(2);
        match self.program.at(at.pc) {
            Instruction::Equal { value, yes, no } => {
                successors.push((yes, cover(words.clone().filter(|w| *w == value))));
                successors.push((no, cover(words.filter(|w| *w != value))));
            }
            Instruction::Less { limit, yes, no } => {
                successors.push((yes, cover(words.clone().filter(|w| *w < limit))));
                successors.push((no, cover(words.filter(|w| *w >= limit))));
            }
            Instruction::Add { amount, next } => {
                successors.push((
                    next,
                    cover(words.map(|w| w.saturating_add(amount).min(MAX))),
                ));
            }
            Instruction::Return | Instruction::Fault => {}
        }
        for (pc, mask) in successors {
            <Cx as Accepts<Parities, There<Here>>>::accept(cx, Scoped::new(at.scope, pc), mask)?;
        }
        Ok(LocalClosure::Stable)
    }
}
