//! 先选择协议版本，再解码和分析；同一个字节在不同 fork 下可能有不同语义。
//!
//! 2026-10-02 核验的最新主网执行层是 Osaka（Fusaka）。revm 已包含
//! Amsterdam 的开发中规则，不能因此把它自动当作主网默认值。
//! 本类型只允许已支持的三个版本；Program 保存版本，CFG、SSA 和输出共享它。

use revm_bytecode::primitives::hardfork::SpecId;
use serde::Serialize;
use std::{fmt, str::FromStr};
use thiserror::Error;

/// 本实验支持的已发布主网执行层规则；默认值在代码中固定，不随网络漂移。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fork {
    /// Dencun 的执行层，2024-03-13 激活；CLZ 尚未启用。
    Cancun,
    /// Pectra 的执行层，2025-05-07 激活；加入 EIP-7702 代码委托。
    Prague,
    /// Fusaka 的执行层，2025-12-03 激活；加入 EIP-7939 CLZ。
    #[default]
    Osaka,
}

impl Fork {
    /// 精确的 revm 版本，用来将同一选择传给独立的具体执行 oracle。
    pub const fn spec_id(self) -> SpecId {
        match self {
            Self::Cancun => SpecId::CANCUN,
            Self::Prague => SpecId::PRAGUE,
            Self::Osaka => SpecId::OSAKA,
        }
    }

    pub(crate) const fn supports_clz(self) -> bool {
        matches!(self, Self::Osaka)
    }
    pub(crate) const fn supports_delegation(self) -> bool {
        !matches!(self, Self::Cancun)
    }
}

impl fmt::Display for Fork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cancun => "cancun",
            Self::Prague => "prague",
            Self::Osaka => "osaka",
        })
    }
}

/// 未实现的 fork 必须明确拒绝，不能悄悄套用另一套规则。
#[derive(Debug, Error)]
#[error("unsupported fork {input:?}; expected cancun, prague or osaka")]
pub struct ParseForkError {
    /// 用户提供的原始版本名称。
    pub input: String,
}

impl FromStr for Fork {
    type Err = ParseForkError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "cancun" => Ok(Self::Cancun),
            "prague" => Ok(Self::Prague),
            "osaka" => Ok(Self::Osaka),
            _ => Err(ParseForkError {
                input: input.to_owned(),
            }),
        }
    }
}
