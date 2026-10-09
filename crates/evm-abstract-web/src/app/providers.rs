//! RPC catalog state is independent of analysis admission, polling and results.

use egui::{RichText, Ui};
use evm_abstract_protocol::{RpcProvider, RpcProvidersReply};

use super::{Command, TransportError, job::TaskError};
use crate::palette;

#[derive(Default)]
enum State {
    #[default]
    Unrequested,
    Loading,
    Ready(Vec<RpcProvider>),
    Failed(TaskError),
}

#[derive(Default)]
pub(super) struct RpcProviders {
    state: State,
    generation: u64,
}

impl RpcProviders {
    pub(super) fn next_command(&mut self) -> Option<Command> {
        if !matches!(self.state, State::Unrequested) {
            return None;
        }
        self.generation += 1;
        self.state = State::Loading;
        Some(Command::RpcProviders {
            generation: self.generation,
        })
    }

    pub(super) fn receive(
        &mut self,
        generation: u64,
        result: Result<RpcProvidersReply, TransportError>,
        selected: &mut String,
    ) {
        if generation != self.generation || !matches!(self.state, State::Loading) {
            return;
        }
        self.state = match result {
            Ok(RpcProvidersReply {
                result: Ok(providers),
            }) => {
                if !providers.iter().any(|provider| provider.id == *selected) {
                    *selected = providers
                        .first()
                        .map(|provider| provider.id.clone())
                        .unwrap_or_default();
                }
                State::Ready(providers)
            }
            Ok(RpcProvidersReply { result: Err(error) }) => State::Failed(TaskError::Api(error)),
            Err(error) => State::Failed(TaskError::Transport(error)),
        };
    }

    pub(super) fn valid_selection(&self, selected: &str) -> bool {
        !selected.is_empty()
            && matches!(&self.state, State::Ready(providers) if providers.iter().any(|provider| provider.id == selected))
    }

    pub(super) fn status(&self, selected: &str) -> String {
        match &self.state {
            State::Unrequested | State::Loading => "Loading RPC providers…".into(),
            State::Ready(providers) if providers.is_empty() => {
                "No RPC providers are configured on the server.".into()
            }
            State::Ready(providers) => providers
                .iter()
                .find(|provider| provider.id == selected)
                .map_or_else(
                    || "Select an RPC provider".into(),
                    |provider| format!("RPC provider: {}", provider_label(provider)),
                ),
            State::Failed(error) => format!("Could not load RPC providers: {error}"),
        }
    }

    pub(super) fn show(&mut self, ui: &mut Ui, selected: &mut String) {
        ui.label(RichText::new("RPC provider").color(palette::MUTED));
        match &self.state {
            State::Ready(providers) if !providers.is_empty() => {
                let label = providers
                    .iter()
                    .find(|provider| provider.id == *selected)
                    .map_or_else(|| "Select an RPC provider".into(), provider_label);
                egui::ComboBox::from_id_salt("rpc_provider")
                    .width(ui.available_width())
                    .selected_text(&label)
                    .truncate()
                    .show_ui(ui, |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        for provider in providers {
                            let label = provider_label(provider);
                            ui.selectable_value(selected, provider.id.clone(), &label)
                                .on_hover_text(label);
                        }
                    })
                    .response
                    .on_hover_text(label);
            }
            State::Unrequested | State::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(self.status(selected));
                });
            }
            State::Ready(_) | State::Failed(_) => {
                ui.colored_label(palette::WARNING, self.status(selected));
                if ui.small_button("Retry").clicked() {
                    self.state = State::Unrequested;
                }
            }
        }
    }
}

fn provider_label(provider: &RpcProvider) -> String {
    format!("{} ({})", provider.name, provider.endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{JobOperation, Message, Workspace};
    use evm_abstract_protocol::{
        AnalysisInput, AnalysisProgress, JobId, JobReply, JobSnapshot, JobState,
    };

    #[test]
    fn catalog_fetch_and_failure_do_not_block_initial_submission_polling_or_cancellation() {
        let mut workspace = Workspace::default();
        assert!(matches!(
            workspace.rpc_provider_command(),
            Some(Command::RpcProviders { generation: 1 })
        ));
        let Command::Submit {
            generation,
            request,
        } = workspace.initial_command()
        else {
            panic!("initial bytecode does not wait for catalog loading")
        };
        assert!(matches!(request.input, AnalysisInput::Bytecode(_)));
        workspace.receive_message(Message::Status {
            generation,
            operation: JobOperation::Submit,
            result: Ok(JobReply {
                result: Ok(JobSnapshot {
                    id: JobId(17),
                    state: JobState::Running,
                    progress: AnalysisProgress::default(),
                }),
            }),
        });
        assert!(workspace.task.next_command(0.0).is_none());
        assert!(matches!(
            workspace.task.next_command(0.4),
            Some(Command::Poll { id: JobId(17), .. })
        ));
        workspace.receive_message(Message::RpcProviders {
            generation: 1,
            result: Err(TransportError::Timeout { seconds: 15 }),
        });
        assert!(
            workspace
                .accessible_rpc_provider_status()
                .starts_with("Could not load RPC providers:")
        );
        assert!(workspace.accessible_status().starts_with("Running"));
        assert!(matches!(
            workspace.task.cancel(),
            Some(Command::Cancel { id: JobId(17), .. })
        ));
    }
}
