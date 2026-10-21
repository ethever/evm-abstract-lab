//! Independently implemented interval analyzer; does not inspect the mask policy.
use super::{
    BoundsFacts, Contradiction, KernelError, LocalClosure, MaskFacts, PublishError, Scope, Scoped,
    Stop, Trace,
};
use analysis_kernel::{Accepts, Charges, Here, Read, Reduction, There};

pub struct Bounds {
    pub trace: Trace,
}
pub struct Task(pub Scope);
impl<Cx> Reduction<Cx> for Bounds
where
    Cx: Read<MaskFacts, Here, Error = KernelError<Stop>>
        + Read<BoundsFacts, There<Here>, Error = KernelError<Stop>>
        + Accepts<BoundsFacts, There<Here>, Error = PublishError<Contradiction, Stop>>
        + Charges<Error = KernelError<Stop>>,
{
    type Task = Task;
    type Error = PublishError<Contradiction, Stop>;
    fn reduce(&mut self, task: &Task, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.charge(256)?;
        let masks = <Cx as Read<MaskFacts, Here>>::query(cx, Scoped::new(task.0, ()))?;
        let bounds = <Cx as Read<BoundsFacts, There<Here>>>::query(cx, Scoped::new(task.0, ()))?;
        if let (Some(mask), Some(&(low, high))) = (masks.value(), bounds.value()) {
            let mut candidates =
                (low..=high).filter(|v| v & mask.ones == mask.ones && v & mask.zeros == 0);
            let first = candidates
                .next()
                .ok_or(PublishError::Policy(Contradiction))?;
            let last = candidates.next_back().unwrap_or(first);
            self.trace
                .borrow_mut()
                .push(("bounds", u16::from(first), u16::from(last)));
            <Cx as Accepts<BoundsFacts, There<Here>>>::accept(
                cx,
                Scoped::new(task.0, ()),
                (first, last),
            )?;
        } else {
            self.trace.borrow_mut().push(("bounds-pending", 0, 0));
        }
        Ok(LocalClosure::Stable)
    }
}
