//! Address drafts and their own provider catalog; no analyzer controller is used.
use super::task::{Command, Error};
use crate::{
    controls::{self, Density},
    framework, palette, routing,
};
use evm_abstract_protocol::{BlockSelector, DecompileAddressInput, RpcProvider, RpcProvidersReply};

#[derive(Default)]
enum Catalog {
    #[default]
    Unrequested,
    Loading,
    Ready(Vec<RpcProvider>),
    Failed(Error),
}

pub(super) struct Address {
    address: String,
    provider: String,
    block: String,
    chain: String,
    catalog: Catalog,
    generation: u64,
}

impl Default for Address {
    fn default() -> Self {
        Self {
            address: String::new(),
            provider: String::new(),
            block: "latest".into(),
            chain: String::new(),
            catalog: Catalog::default(),
            generation: 0,
        }
    }
}

impl Address {
    pub fn load(&mut self, input: &DecompileAddressInput) {
        self.address = input.address.clone();
        self.provider = input
            .provider_id
            .clone()
            .unwrap_or_else(|| match &self.catalog {
                Catalog::Ready(providers) => {
                    providers.first().map(|p| p.id.clone()).unwrap_or_default()
                }
                _ => String::new(),
            });
        self.block = match &input.block {
            BlockSelector::Latest => "latest".into(),
            BlockSelector::Number(number) => number.to_string(),
            BlockSelector::Hash(hash) => hash.clone(),
        };
        self.chain = input.chain_id.clone().unwrap_or_default();
    }

    pub fn resolved_provider(&mut self, provider: &str) {
        self.provider = provider.to_owned();
    }

    pub fn next(&mut self) -> Option<Command> {
        if !matches!(self.catalog, Catalog::Unrequested) {
            return None;
        }
        self.generation = self
            .generation
            .checked_add(1)
            .expect("provider generation exhausted");
        self.catalog = Catalog::Loading;
        Some(Command::Providers {
            generation: self.generation,
        })
    }

    pub fn receive(
        &mut self,
        generation: u64,
        reply: Result<RpcProvidersReply, crate::TransportError>,
    ) {
        if generation != self.generation || !matches!(self.catalog, Catalog::Loading) {
            return;
        }
        self.catalog = match reply
            .map_err(Error::Transport)
            .and_then(|reply| reply.result.map_err(Error::Api))
        {
            Ok(providers) => {
                if self.provider.is_empty() {
                    self.provider = providers.first().map(|p| p.id.clone()).unwrap_or_default();
                }
                Catalog::Ready(providers)
            }
            Err(error) => Catalog::Failed(error),
        };
    }

    pub fn validation(&self) -> Option<&'static str> {
        if routing::fixed_hex(self.address.trim(), 20).is_none() {
            return Some("Enter a 20-byte contract address");
        }
        if routing::parse_block(self.block.trim()).is_err() {
            return Some("Block: use latest, a number, or a block hash");
        }
        if !self.chain.trim().is_empty() && routing::parse_chain(self.chain.trim()).is_err() {
            return Some("Chain ID: use an unsigned decimal number");
        }
        match &self.catalog {
            Catalog::Ready(providers) if providers.iter().any(|p| p.id == self.provider) => None,
            Catalog::Ready(providers) if providers.is_empty() => {
                Some("Configure an RPC provider on the server or use runtime bytecode")
            }
            Catalog::Ready(_) => Some("Select an available RPC provider"),
            Catalog::Unrequested | Catalog::Loading => Some("Loading RPC providers"),
            Catalog::Failed(_) => Some("Retry loading RPC providers or use runtime bytecode"),
        }
    }

    pub fn input(&self) -> Option<DecompileAddressInput> {
        self.validation().is_none().then_some(())?;
        Some(DecompileAddressInput {
            address: format!("0x{}", routing::fixed_hex(self.address.trim(), 20)?),
            provider_id: Some(self.provider.clone()),
            block: routing::parse_block(self.block.trim()).ok()?,
            chain_id: (!self.chain.trim().is_empty())
                .then(|| routing::parse_chain(self.chain.trim()))
                .transpose()
                .ok()?,
        })
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.label("Contract address");
        framework::line_editor(ui, "Contract address", &mut self.address, "0x…");
        ui.horizontal_wrapped(|ui| {
            ui.label("RPC provider");
            match &self.catalog {
                Catalog::Ready(providers) if !providers.is_empty() => {
                    let selected = providers
                        .iter()
                        .find(|p| p.id == self.provider)
                        .map(|p| p.name.as_str())
                        .unwrap_or("Select an RPC provider");
                    let response = egui::ComboBox::from_id_salt("decompiler_provider")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for provider in providers {
                                controls::tab_value(
                                    ui,
                                    &mut self.provider,
                                    provider.id.clone(),
                                    &provider.name,
                                    Density::Regular,
                                );
                            }
                        })
                        .response;
                    controls::observe(ui, &response, "RPC provider", "combobox", None);
                }
                Catalog::Unrequested | Catalog::Loading => {
                    ui.spinner();
                    ui.label("Loading RPC providers…");
                }
                Catalog::Ready(_) => {
                    ui.colored_label(
                        palette::WARNING,
                        "No RPC providers are configured on the server.",
                    );
                }
                Catalog::Failed(error) => {
                    ui.colored_label(palette::ERROR, error.to_string());
                }
            }
            if matches!(self.catalog, Catalog::Ready(_) | Catalog::Failed(_))
                && controls::action(ui, "Reload RPC providers", Density::Compact).clicked()
            {
                self.catalog = Catalog::Unrequested;
            }
        });
        // Each editor consumes the available width. Give it a separate row so
        // the following source field remains visible at every viewport width.
        ui.label("Block");
        framework::line_editor(
            ui,
            "Source block",
            &mut self.block,
            "latest, number, or hash",
        );
        ui.label("Expected chain ID");
        framework::line_editor(ui, "Expected chain ID", &mut self.chain, "optional");
        ui.label(
            egui::RichText::new("The selected block is pinned before the runtime code is read.")
                .small()
                .color(palette::MUTED),
        );
    }
}
