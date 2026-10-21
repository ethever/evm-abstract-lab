//! The external World input read owns one actual payload clone.

use super::{EvmMachinePayload, MachineError, Runtime, WordElement, WorkBudget, kernel_failure};
use analysis_kernel::{Answer, RunningJob, SlotMeta};

pub(super) fn read<N: WordElement, T>(
    runtime: &mut Runtime<T>,
    active: &mut RunningJob,
    slot: &SlotMeta<EvmMachinePayload<N>>,
    value: &EvmMachinePayload<N>,
    budget: &mut WorkBudget,
) -> Result<Answer<EvmMachinePayload<N>>, MachineError> {
    // Pay the bounded census before the kernel reserves this clone. Retention,
    // admission and the tracked input identity keep their existing contracts.
    let clone_work = value.clone_work_with(budget)?;
    runtime.with_run(active, budget, |run| {
        run.read_slot(slot, value, clone_work)
            .map_err(kernel_failure)
    })
}

#[cfg(test)]
mod tests;
