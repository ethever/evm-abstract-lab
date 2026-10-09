//! Choose the initial example after the provider catalog has resolved. A user
//! opening the editor or submitting a task takes control of subsequent work.

use super::{Command, Workspace, job::TaskPhase};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Startup {
    #[default]
    Unstarted,
    WaitingForProviders(u64),
    Ready,
    Finished,
}

impl Startup {
    pub(super) fn loading(self) -> bool {
        matches!(self, Self::WaitingForProviders(_))
    }

    pub(super) fn receive_catalog(&mut self, generation: u64) {
        if *self == Self::WaitingForProviders(generation) {
            *self = Self::Ready;
        }
    }
}

impl Workspace {
    /// Begin one automatic example selection. Available RPC providers select
    /// the default USDC input; an empty or unavailable catalog uses bytecode.
    pub fn startup_command(&mut self) -> Option<Command> {
        if self.startup != Startup::Unstarted {
            return None;
        }
        if self.form.open || self.task.phase != TaskPhase::Idle || self.report.is_some() {
            self.startup = Startup::Finished;
            return None;
        }
        let command = self.form.provider_command();
        self.startup = self
            .form
            .provider_loading_generation()
            .map_or(Startup::Ready, Startup::WaitingForProviders);
        command.or_else(|| self.pending_startup_command())
    }

    pub(super) fn pending_startup_command(&mut self) -> Option<Command> {
        if self.startup != Startup::Ready {
            return None;
        }
        self.startup = Startup::Finished;
        if self.form.open || self.task.phase != TaskPhase::Idle || self.report.is_some() {
            return None;
        }
        self.form.select_rpc_example_if_available();
        Some(self.initial_command())
    }
}

#[cfg(test)]
mod tests;
