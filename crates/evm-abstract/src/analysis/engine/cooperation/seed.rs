//! Replay internal initialization after a dependency/readiness wake. Native slot
//! premises are cumulative in this selected scope; changing them retires it.
use super::{Composition, Cooperation};
use crate::{
    analysis::{
        cooperation::{
            self as public, CooperationError, InputTasks, PointKind, Ports, ProcedureId, Subject,
        },
        worklist::{MachineMeter, runtime::Runtime},
    },
    domain::WordElement,
    resource::WorkBudget,
};
use analysis_kernel::{Availability, JobId, KernelError, LocalClosure, Meter, RunningJob, Scoped};

impl<N: WordElement, X: Composition<N, Error = E>, E> Cooperation<N, X, E> {
    pub(super) fn evaluate_seed(
        &mut self,
        runtime: &mut Runtime<<X as InputTasks>::Task>,
        subject: Subject,
        mut token: RunningJob,
        budget: &mut WorkBudget,
        draining: Option<JobId>,
    ) -> Result<(), CooperationError<E>> {
        MachineMeter(budget)
            .reserve(((usize::BITS as usize) + 1) * 64)
            .map_err(KernelError::Stopped)?;
        let readonly = subject.point.kind == PointKind::ProcedureEntry;
        let (input, parent, owner) = if readonly {
            let binding = self
                .readonly
                .get(&ProcedureId {
                    namespace: subject.point.namespace,
                    index: subject.point.index,
                })
                .filter(|binding| binding.input.subject() == subject)
                .ok_or(KernelError::InvalidHandle)?;
            (binding.input.clone(), None, binding.owner)
        } else {
            let binding = self
                .bindings
                .get(&subject.point)
                .filter(|binding| binding.input.subject() == subject)
                .ok_or(KernelError::InvalidHandle)?;
            (
                binding.input.clone(),
                binding.stamp.parent,
                binding.consumer,
            )
        };
        // A retired scope is a valid obsolete receipt, but cannot be queried.
        // Check it through the metered receipt API before using the typed route.
        let parent_stale = match parent {
            Some(parent) => runtime
                .view()
                .scope_stale(parent.scope, &mut MachineMeter(budget))?,
            None => false,
        };
        let mut meter = MachineMeter(budget);
        let mut run = runtime.kernel().resume(&mut token, &mut meter)?;
        if run.scope() != subject.scope {
            return Err(KernelError::WrongScope.into());
        }
        let shared = self.shared.as_mut().ok_or(KernelError::InvalidHandle)?;
        let ready = if parent_stale {
            false
        } else if let Some(parent) = parent {
            // Read the captured source, never the latest selector. An unavailable
            // parent cannot authorize this previously prepared native payload.
            let answer = run.query(&mut shared.inputs, Scoped::new(parent.scope, ()))?;
            matches!(answer.state, Availability::Value(_))
        } else {
            true
        };
        if ready {
            let published = run.query(&mut shared.inputs, Scoped::new(subject.scope, ()))?;
            if !matches!(published.state, Availability::Value(_)) {
                public::initialize_readonly(&mut run, shared, &input)?;
            }
        }
        if ready && !readonly {
            let feedback = run.query(&mut shared.refinements, Scoped::new(subject.scope, ()))?;
            if !matches!(feedback.state, Availability::Value(_)) {
                let mut ports = Ports {
                    shared,
                    local: self
                        .stores
                        .get_mut(&subject.scope)
                        .ok_or(KernelError::InvalidHandle)?,
                    global: self.global.as_mut().ok_or(KernelError::InvalidHandle)?,
                };
                public::initialize_feedback(&mut run, &mut ports, subject)?;
            }
        }
        let stale = {
            let (kernel, meter) = run.parts();
            meter.checkpoint().map_err(KernelError::Stopped)?;
            kernel.scope_stale(subject.scope, meter)?
        };
        if stale {
            return Ok(());
        }
        run.finish(LocalClosure::Stable)?;
        if !ready {
            self.defer_owner(owner, runtime, meter.0)?;
            // If an unseeded scope never adopted the parent's head, parent
            // retirement need not retire it. Re-enter its real native owner to
            // replace/withdraw it using a fresh execution receipt instead.
            if parent_stale {
                match runtime.kernel().retry(owner, &mut meter) {
                    Ok(()) | Err(KernelError::AlreadyRunning) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        self.wake_ready(runtime, subject.scope, meter.0, draining)
    }
}
