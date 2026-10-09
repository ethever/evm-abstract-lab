//! Mainnet entry examples are address edits, independent of RPC configuration.
//! Selection is derived from the draft so manual edits remain authoritative.

use egui::{Id, RichText, Ui};

use crate::{framework, palette};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Example {
    Usdc,
    Weth,
}

impl Example {
    const ALL: [Self; 2] = [Self::Usdc, Self::Weth];

    pub(super) fn address(self) -> &'static str {
        match self {
            Self::Usdc => "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
            Self::Weth => "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Usdc => "USDC",
            Self::Weth => "WETH",
        }
    }

    fn matching(address: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|example| example.address().eq_ignore_ascii_case(address))
    }
}

pub(super) fn root_account(ui: &mut Ui, address: &mut String) {
    let selected = Example::matching(address);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Root account").color(palette::MUTED));
        egui::ComboBox::from_id_salt("rpc_entry_example")
            .width(76.0)
            .selected_text(selected.map_or("Custom", Example::label))
            .show_ui(ui, |ui| {
                ui.label(
                    RichText::new("Ethereum mainnet")
                        .small()
                        .color(palette::MUTED),
                );
                for example in Example::ALL {
                    if ui
                        .selectable_label(selected == Some(example), example.label())
                        .on_hover_text(example.address())
                        .clicked()
                    {
                        *address = example.address().into();
                        ui.close();
                    }
                }
                ui.separator();
                if ui.selectable_label(selected.is_none(), "Custom").clicked() {
                    if selected.is_some() {
                        address.clear();
                    }
                    ui.memory_mut(|memory| memory.request_focus(Id::new("rpc_address")));
                    ui.close();
                }
            });
        if selected.is_some() {
            ui.label(
                RichText::new("Ethereum mainnet")
                    .small()
                    .color(palette::MUTED),
            )
            .on_hover_text(
                "Choose an RPC provider for Ethereum mainnet when using these example addresses.",
            );
        }
    });
    framework::line_editor(ui, "rpc_address", address, "0x… (20 bytes)");
}
