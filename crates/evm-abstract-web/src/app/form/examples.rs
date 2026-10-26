//! Mainnet entry examples are address edits, independent of RPC configuration.
//! Selection is derived from the draft so manual edits remain authoritative.

use crate::controls::{self, Density};
use egui::{Id, RichText, Ui};

use crate::{framework, palette};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Example {
    Usdc,
    Weth,
    Usdt,
    Multicall3,
    UniswapV2Router02,
    UniswapV2Pair,
    UniswapV2Factory,
    UniswapV4PoolManager,
}

impl Example {
    const ALL: [Self; 8] = [
        Self::Usdc,
        Self::Weth,
        Self::Usdt,
        Self::Multicall3,
        Self::UniswapV2Router02,
        Self::UniswapV2Pair,
        Self::UniswapV2Factory,
        Self::UniswapV4PoolManager,
    ];

    pub(super) fn address(self) -> &'static str {
        match self {
            Self::Usdc => "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
            Self::Weth => "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2",
            Self::Usdt => "0xdAC17F958D2ee523a2206206994597C13D831ec7",
            Self::Multicall3 => "0xcA11bde05977b3631167028862bE2a173976CA11",
            Self::UniswapV2Router02 => "0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D",
            Self::UniswapV2Pair => "0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc",
            Self::UniswapV2Factory => "0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f",
            Self::UniswapV4PoolManager => "0x000000000004444c5dc75cB358380D2e3dE08A90",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Usdc => "USDC",
            Self::Weth => "WETH",
            Self::Usdt => "USDT",
            Self::Multicall3 => "Multicall3",
            Self::UniswapV2Router02 => "Uniswap V2 Router02",
            Self::UniswapV2Pair => "Uniswap V2 Pair (USDC/WETH)",
            Self::UniswapV2Factory => "Uniswap V2 Factory",
            Self::UniswapV4PoolManager => "Uniswap V4 PoolManager",
        }
    }

    fn hover_text(self) -> String {
        let mut text = self.address().to_owned();
        if self == Self::UniswapV4PoolManager {
            text.push_str("\nUniswap V4 pools live in this singleton PoolManager.");
        }
        text
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
        let response = egui::ComboBox::from_id_salt("rpc_entry_example")
            // A wrapping row's available_width includes the whole next row.
            // The nested ComboBox needs the space remaining beside this label
            // or it can grow the centered modal beyond a narrow viewport.
            .width(240.0_f32.min(ui.available_size_before_wrap().x).max(80.0))
            .height(300.0)
            .wrap()
            .selected_text(selected.map_or("Custom", Example::label))
            .show_ui(ui, |ui| {
                ui.label(
                    RichText::new("Ethereum mainnet")
                        .small()
                        .color(palette::MUTED),
                );
                for example in Example::ALL {
                    if controls::tab(
                        ui,
                        selected == Some(example),
                        example.label(),
                        Density::Regular,
                    )
                    .on_hover_text(example.hover_text())
                    .clicked()
                    {
                        *address = example.address().into();
                        ui.close();
                    }
                }
                ui.separator();
                if controls::tab(ui, selected.is_none(), "Custom", Density::Regular).clicked() {
                    if selected.is_some() {
                        address.clear();
                    }
                    ui.memory_mut(|memory| memory.request_focus(Id::new("rpc_address")));
                    ui.close();
                }
            })
            .response;
        if let Some(example) = selected {
            response.on_hover_text(example.hover_text());
        }
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
