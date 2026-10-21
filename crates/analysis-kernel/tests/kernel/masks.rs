//! Independently implemented mask analyzer; only typed capabilities cross modules.
use super::{
    BoundsFacts, Contradiction, KernelError, LocalClosure, Mask, MaskFacts, PublishError, Scope,
    Scoped, Stop, Trace,
};
use analysis_kernel::{Accepts, Charges, Here, Read, Reduction, There};

pub struct Masks {
    pub trace: Trace,
}
pub struct Task(pub Scope);
impl<Cx> Reduction<Cx> for Masks
where
    Cx: Read<BoundsFacts, There<Here>, Error = KernelError<Stop>>
        + Accepts<MaskFacts, Here, Error = PublishError<Contradiction, Stop>>
        + Charges<Error = KernelError<Stop>>,
{
    type Task = Task;
    type Error = PublishError<Contradiction, Stop>;
    fn reduce(&mut self, task: &Task, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.charge(16)?;
        let bounds = <Cx as Read<BoundsFacts, There<Here>>>::query(cx, Scoped::new(task.0, ()))?;
        if let Some(&(low, high)) = bounds.value() {
            let changed_bits = low ^ high;
            let differing = if changed_bits == 0 {
                0
            } else {
                u8::MAX >> changed_bits.leading_zeros()
            };
            let mask = Mask {
                ones: low & !differing,
                zeros: !low & !differing,
            };
            self.trace
                .borrow_mut()
                .push(("masks", u16::from(mask.ones), u16::from(mask.zeros)));
            <Cx as Accepts<MaskFacts, Here>>::accept(cx, Scoped::new(task.0, ()), mask)?;
        } else {
            self.trace.borrow_mut().push(("masks-pending", 0, 0));
        }
        Ok(LocalClosure::Stable)
    }
}
